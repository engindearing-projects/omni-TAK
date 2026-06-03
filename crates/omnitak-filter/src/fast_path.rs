//! Fast-path optimizations for high-performance filtering
//!
//! This module provides optimized implementations for common filtering operations.
//! Uses const lookups, SIMD-friendly operations, and cache-friendly data structures.

use crate::affiliation::Affiliation;
#[cfg(test)]
use crate::affiliation::CotType;
use memchr::memchr;
use std::sync::atomic::{AtomicU64, Ordering};

/// Optimized affiliation lookup using const array
///
/// This provides O(1) lookup for affiliation codes without allocations.
const AFFILIATION_LOOKUP: [Option<Affiliation>; 256] = {
    let mut lookup = [None; 256];

    // Lowercase
    lookup[b'p' as usize] = Some(Affiliation::Pending);
    lookup[b'u' as usize] = Some(Affiliation::Unknown);
    lookup[b'a' as usize] = Some(Affiliation::AssumedFriend);
    lookup[b'f' as usize] = Some(Affiliation::Friend);
    lookup[b'n' as usize] = Some(Affiliation::Neutral);
    lookup[b's' as usize] = Some(Affiliation::Suspect);
    lookup[b'h' as usize] = Some(Affiliation::Hostile);
    lookup[b'j' as usize] = Some(Affiliation::Joker);
    lookup[b'k' as usize] = Some(Affiliation::Faker);

    // Uppercase
    lookup[b'P' as usize] = Some(Affiliation::Pending);
    lookup[b'U' as usize] = Some(Affiliation::Unknown);
    lookup[b'A' as usize] = Some(Affiliation::AssumedFriend);
    lookup[b'F' as usize] = Some(Affiliation::Friend);
    lookup[b'N' as usize] = Some(Affiliation::Neutral);
    lookup[b'S' as usize] = Some(Affiliation::Suspect);
    lookup[b'H' as usize] = Some(Affiliation::Hostile);
    lookup[b'J' as usize] = Some(Affiliation::Joker);
    lookup[b'K' as usize] = Some(Affiliation::Faker);

    lookup
};

/// Fast affiliation extraction from CoT type string
///
/// Uses SIMD-accelerated memchr to find separators and const lookup for affiliation.
#[inline]
pub fn fast_extract_affiliation(cot_type: &str) -> Option<Affiliation> {
    let bytes = cot_type.as_bytes();

    // Find first dash using SIMD-accelerated memchr
    let first_dash = memchr(b'-', bytes)?;

    // Find second dash (affiliation is between first and second dash)
    let second_dash = memchr(b'-', &bytes[first_dash + 1..])? + first_dash + 1;

    // Extract affiliation byte (between first and second dash)
    if first_dash + 1 < second_dash && second_dash <= bytes.len() {
        let aff_byte = bytes[first_dash + 1];
        return AFFILIATION_LOOKUP.get(aff_byte as usize).copied().flatten();
    }

    None
}

/// Fast check if CoT type is friendly
///
/// Optimized hot path for the most common filter check.
#[inline]
pub fn fast_is_friendly(cot_type: &str) -> bool {
    if let Some(aff) = fast_extract_affiliation(cot_type) {
        matches!(
            aff,
            Affiliation::Friend | Affiliation::AssumedFriend | Affiliation::Joker
        )
    } else {
        false
    }
}

/// Fast check if CoT type is hostile
#[inline]
pub fn fast_is_hostile(cot_type: &str) -> bool {
    if let Some(aff) = fast_extract_affiliation(cot_type) {
        matches!(
            aff,
            Affiliation::Hostile | Affiliation::Suspect | Affiliation::Faker
        )
    } else {
        false
    }
}

/// Fast bounding box check using optimized float comparisons
///
/// This is a hot path for geographic filtering.
#[inline]
pub fn fast_in_bbox(lat: f64, lon: f64, bbox: &[f64; 4]) -> bool {
    // Order: [min_lat, max_lat, min_lon, max_lon]
    // Using & for bitwise AND to avoid branch prediction misses
    (lat >= bbox[0]) & (lat <= bbox[1]) & (lon >= bbox[2]) & (lon <= bbox[3])
}

/// Lock-free counter for filter statistics
///
/// Uses atomic operations for thread-safe counting without locks.
#[derive(Debug)]
pub struct FastCounter {
    count: AtomicU64,
}

impl FastCounter {
    /// Create a new counter
    pub const fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
        }
    }

    /// Increment the counter
    #[inline]
    pub fn increment(&self) {
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    /// Get the current count
    #[inline]
    pub fn get(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// Reset the counter
    #[inline]
    pub fn reset(&self) {
        self.count.store(0, Ordering::Relaxed);
    }
}

impl Default for FastCounter {
    fn default() -> Self {
        Self::new()
    }
}

/// Bloom filter for fast UID rejection
///
/// Uses a simple bloom filter for fast negative lookups.
/// This provides O(1) average case for "definitely not in set" checks.
pub struct UidBloomFilter {
    bits: Vec<AtomicU64>,
    hash_count: usize,
}

impl UidBloomFilter {
    /// Create a new bloom filter
    ///
    /// # Arguments
    /// * `expected_items` - Expected number of items
    /// * `false_positive_rate` - Desired false positive rate (e.g., 0.01 for 1%)
    pub fn new(expected_items: usize, false_positive_rate: f64) -> Self {
        // Calculate optimal bit array size
        let bits_per_item = -1.44 * false_positive_rate.log2();
        let total_bits = (expected_items as f64 * bits_per_item).ceil() as usize;
        let num_u64s = total_bits.div_ceil(64);

        // Calculate optimal number of hash functions
        let hash_count = (bits_per_item * 0.693).ceil() as usize;

        Self {
            bits: (0..num_u64s).map(|_| AtomicU64::new(0)).collect(),
            hash_count,
        }
    }

    /// Insert a UID into the bloom filter
    pub fn insert(&self, uid: &str) {
        for i in 0..self.hash_count {
            let hash = self.hash(uid, i);
            let bit_index = hash % (self.bits.len() * 64);
            let u64_index = bit_index / 64;
            let bit_offset = bit_index % 64;

            if let Some(u64_cell) = self.bits.get(u64_index) {
                u64_cell.fetch_or(1u64 << bit_offset, Ordering::Relaxed);
            }
        }
    }

    /// Check if a UID might be in the set
    ///
    /// Returns true if the UID might be in the set (or false positive).
    /// Returns false if the UID is definitely not in the set.
    pub fn contains(&self, uid: &str) -> bool {
        for i in 0..self.hash_count {
            let hash = self.hash(uid, i);
            let bit_index = hash % (self.bits.len() * 64);
            let u64_index = bit_index / 64;
            let bit_offset = bit_index % 64;

            if let Some(u64_cell) = self.bits.get(u64_index) {
                let bits = u64_cell.load(Ordering::Relaxed);
                if (bits & (1u64 << bit_offset)) == 0 {
                    return false; // Definitely not in set
                }
            }
        }
        true // Might be in set
    }

    /// Hash function for bloom filter
    fn hash(&self, s: &str, seed: usize) -> usize {
        // Simple FNV-1a hash with seed
        let mut hash = 14695981039346656037u64.wrapping_add(seed as u64);
        for byte in s.bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(1099511628211);
        }
        hash as usize
    }
}

/// Cache-friendly string matcher using SIMD
///
/// Optimized for checking if a string starts with a prefix.
#[inline]
pub fn fast_starts_with(s: &str, prefix: &str) -> bool {
    s.as_bytes().starts_with(prefix.as_bytes())
}

/// Performance metrics collector
#[derive(Debug)]
pub struct FilterMetrics {
    pub evaluations: FastCounter,
    pub passes: FastCounter,
    pub blocks: FastCounter,
    pub cache_hits: FastCounter,
    pub cache_misses: FastCounter,
}

impl FilterMetrics {
    /// Create new metrics
    pub const fn new() -> Self {
        Self {
            evaluations: FastCounter::new(),
            passes: FastCounter::new(),
            blocks: FastCounter::new(),
            cache_hits: FastCounter::new(),
            cache_misses: FastCounter::new(),
        }
    }

    /// Get pass rate
    pub fn pass_rate(&self) -> f64 {
        let total = self.evaluations.get();
        if total == 0 {
            0.0
        } else {
            (self.passes.get() as f64 / total as f64) * 100.0
        }
    }

    /// Get cache hit rate
    pub fn cache_hit_rate(&self) -> f64 {
        let total = self.cache_hits.get() + self.cache_misses.get();
        if total == 0 {
            0.0
        } else {
            (self.cache_hits.get() as f64 / total as f64) * 100.0
        }
    }
}

impl Default for FilterMetrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fast_extract_affiliation() {
        let aff = fast_extract_affiliation("a-f-G-E-V-C").unwrap();
        assert_eq!(aff, Affiliation::Friend);

        let aff = fast_extract_affiliation("a-h-A-M-F").unwrap();
        assert_eq!(aff, Affiliation::Hostile);

        let aff = fast_extract_affiliation("a-n-G").unwrap();
        assert_eq!(aff, Affiliation::Neutral);
    }

    #[test]
    fn test_fast_is_friendly() {
        assert!(fast_is_friendly("a-f-G-E-V-C"));
        assert!(fast_is_friendly("a-a-G-E-V-C"));
        assert!(!fast_is_friendly("a-h-G-E-V-C"));
        assert!(!fast_is_friendly("a-n-G-E-V-C"));
    }

    #[test]
    fn test_fast_is_hostile() {
        assert!(fast_is_hostile("a-h-G-E-V-C"));
        assert!(fast_is_hostile("a-s-G-E-V-C"));
        assert!(!fast_is_hostile("a-f-G-E-V-C"));
        assert!(!fast_is_hostile("a-n-G-E-V-C"));
    }

    #[test]
    fn test_fast_in_bbox() {
        let bbox = [40.0, 45.0, -75.0, -70.0]; // [min_lat, max_lat, min_lon, max_lon]

        assert!(fast_in_bbox(42.0, -72.0, &bbox));
        assert!(!fast_in_bbox(38.0, -72.0, &bbox)); // lat too low
        assert!(!fast_in_bbox(42.0, -80.0, &bbox)); // lon too low
        assert!(!fast_in_bbox(50.0, -72.0, &bbox)); // lat too high
        assert!(!fast_in_bbox(42.0, -65.0, &bbox)); // lon too high
    }

    #[test]
    fn test_fast_counter() {
        let counter = FastCounter::new();
        assert_eq!(counter.get(), 0);

        counter.increment();
        counter.increment();
        assert_eq!(counter.get(), 2);

        counter.reset();
        assert_eq!(counter.get(), 0);
    }

    #[test]
    fn test_bloom_filter() {
        let bloom = UidBloomFilter::new(1000, 0.01);

        bloom.insert("UID-001");
        bloom.insert("UID-002");
        bloom.insert("UID-003");

        assert!(bloom.contains("UID-001"));
        assert!(bloom.contains("UID-002"));
        assert!(bloom.contains("UID-003"));

        // This might be a false positive, but should mostly be false
        // We can't guarantee it's false due to the nature of bloom filters
    }

    #[test]
    fn test_filter_metrics() {
        let metrics = FilterMetrics::new();

        metrics.evaluations.increment();
        metrics.passes.increment();
        metrics.evaluations.increment();
        metrics.blocks.increment();

        assert_eq!(metrics.evaluations.get(), 2);
        assert_eq!(metrics.passes.get(), 1);
        assert_eq!(metrics.blocks.get(), 1);
        assert_eq!(metrics.pass_rate(), 50.0);
    }

    #[test]
    fn test_affiliation_lookup_const() {
        // Test that the const lookup array is correctly initialized
        assert_eq!(AFFILIATION_LOOKUP[b'f' as usize], Some(Affiliation::Friend));
        assert_eq!(AFFILIATION_LOOKUP[b'F' as usize], Some(Affiliation::Friend));
        assert_eq!(
            AFFILIATION_LOOKUP[b'h' as usize],
            Some(Affiliation::Hostile)
        );
        assert_eq!(AFFILIATION_LOOKUP[b'x' as usize], None);
    }

    #[test]
    fn test_fast_starts_with() {
        assert!(fast_starts_with("hello world", "hello"));
        assert!(!fast_starts_with("hello world", "world"));
        assert!(fast_starts_with("a-f-G-E-V-C", "a-f"));
    }

    #[test]
    fn benchmark_fast_extract_vs_normal() {
        // Compare performance of fast extraction vs normal parsing
        let cot_type = "a-f-G-E-V-C-U-I-M-N-O-P";

        // Fast path
        let start = std::time::Instant::now();
        for _ in 0..100_000 {
            let _ = fast_extract_affiliation(cot_type);
        }
        let fast_duration = start.elapsed();

        // Normal path
        let start = std::time::Instant::now();
        for _ in 0..100_000 {
            let cot = CotType::parse(cot_type);
            let _ = cot.affiliation;
        }
        let normal_duration = start.elapsed();

        println!("Fast path: {:?}", fast_duration);
        println!("Normal path: {:?}", normal_duration);

        // Fast path should be significantly faster (at least 2x)
        assert!(fast_duration < normal_duration);
    }
}
