//! MIL-STD-2525 CoT type field parser
//!
//! Parses CoT type fields like "a-f-G-E-V-C-U-I" into structured components.
//! This is a zero-allocation parser optimized for performance.
//!
//! CoT Type Format: [atoms]-[affiliation]-[dimension]-[function]-[...]
//! Example: a-f-G-E-V-C-U-I
//!   atoms: 'a' (pending)
//!   affiliation: 'f' (friendly)
//!   dimension: 'G' (ground)
//!   function: 'E' (equipment)

use std::fmt;

/// Affiliation codes from MIL-STD-2525
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Affiliation {
    /// Pending (p) - Not yet categorized
    Pending,
    /// Unknown (u) - Identity unknown
    Unknown,
    /// Assumed Friend (a) - Assumed to be friendly
    AssumedFriend,
    /// Friend (f) - Confirmed friendly
    Friend,
    /// Neutral (n) - Non-combatant
    Neutral,
    /// Suspect (s) - Possibly hostile
    Suspect,
    /// Hostile (h) - Confirmed hostile
    Hostile,
    /// Joker (j) - Friendly for exercise purposes
    Joker,
    /// Faker (k) - Hostile for exercise purposes
    Faker,
}

impl Affiliation {
    /// Parse affiliation from a single character
    #[inline]
    pub const fn from_char(c: char) -> Option<Self> {
        match c {
            'p' | 'P' => Some(Affiliation::Pending),
            'u' | 'U' => Some(Affiliation::Unknown),
            'a' | 'A' => Some(Affiliation::AssumedFriend),
            'f' | 'F' => Some(Affiliation::Friend),
            'n' | 'N' => Some(Affiliation::Neutral),
            's' | 'S' => Some(Affiliation::Suspect),
            'h' | 'H' => Some(Affiliation::Hostile),
            'j' | 'J' => Some(Affiliation::Joker),
            'k' | 'K' => Some(Affiliation::Faker),
            _ => None,
        }
    }

    /// Check if this affiliation is considered friendly
    #[inline]
    pub const fn is_friendly(&self) -> bool {
        matches!(
            self,
            Affiliation::Friend | Affiliation::AssumedFriend | Affiliation::Joker
        )
    }

    /// Check if this affiliation is considered hostile
    #[inline]
    pub const fn is_hostile(&self) -> bool {
        matches!(
            self,
            Affiliation::Hostile | Affiliation::Suspect | Affiliation::Faker
        )
    }

    /// Check if this affiliation is neutral
    #[inline]
    pub const fn is_neutral(&self) -> bool {
        matches!(self, Affiliation::Neutral)
    }
}

impl fmt::Display for Affiliation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Affiliation::Pending => write!(f, "Pending"),
            Affiliation::Unknown => write!(f, "Unknown"),
            Affiliation::AssumedFriend => write!(f, "Assumed Friend"),
            Affiliation::Friend => write!(f, "Friend"),
            Affiliation::Neutral => write!(f, "Neutral"),
            Affiliation::Suspect => write!(f, "Suspect"),
            Affiliation::Hostile => write!(f, "Hostile"),
            Affiliation::Joker => write!(f, "Joker"),
            Affiliation::Faker => write!(f, "Faker"),
        }
    }
}

/// Dimension codes from MIL-STD-2525
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dimension {
    /// Space
    Space,
    /// Air
    Air,
    /// Ground
    Ground,
    /// Sea Surface
    SeaSurface,
    /// Sea Subsurface
    SeaSubsurface,
    /// Special Operations Forces
    SOF,
    /// Other
    Other,
}

impl Dimension {
    /// Parse dimension from a single character
    #[inline]
    pub const fn from_char(c: char) -> Option<Self> {
        match c {
            'P' | 'p' => Some(Dimension::Space),
            'A' | 'a' => Some(Dimension::Air),
            'G' | 'g' => Some(Dimension::Ground),
            'S' | 's' => Some(Dimension::SeaSurface),
            'U' | 'u' => Some(Dimension::SeaSubsurface),
            'F' | 'f' => Some(Dimension::SOF),
            _ => Some(Dimension::Other),
        }
    }
}

impl fmt::Display for Dimension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Dimension::Space => write!(f, "Space"),
            Dimension::Air => write!(f, "Air"),
            Dimension::Ground => write!(f, "Ground"),
            Dimension::SeaSurface => write!(f, "Sea Surface"),
            Dimension::SeaSubsurface => write!(f, "Sea Subsurface"),
            Dimension::SOF => write!(f, "SOF"),
            Dimension::Other => write!(f, "Other"),
        }
    }
}

/// Parsed CoT type information
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CotType<'a> {
    /// Raw CoT type string (zero-copy reference)
    pub raw: &'a str,
    /// Atoms/pending status
    pub atoms: Option<char>,
    /// Affiliation (friendly, hostile, etc.)
    pub affiliation: Option<Affiliation>,
    /// Dimension (ground, air, sea, etc.)
    pub dimension: Option<Dimension>,
    /// Function code (equipment, vehicle, etc.)
    pub function: Option<&'a str>,
}

impl<'a> CotType<'a> {
    /// Parse a CoT type string with zero allocations
    ///
    /// # Examples
    /// ```
    /// use omnitak_filter::affiliation::CotType;
    ///
    /// let cot = CotType::parse("a-f-G-E-V-C-U-I");
    /// assert!(cot.affiliation.unwrap().is_friendly());
    /// ```
    #[inline]
    pub fn parse(type_str: &'a str) -> Self {
        let mut parts = type_str.split('-');

        let atoms = parts.next().and_then(|s| s.chars().next());
        let affiliation = parts
            .next()
            .and_then(|s| s.chars().next())
            .and_then(Affiliation::from_char);
        let dimension = parts
            .next()
            .and_then(|s| s.chars().next())
            .and_then(Dimension::from_char);

        // Remaining parts form the function code
        let function_start = type_str
            .char_indices()
            .filter(|(_, c)| *c == '-')
            .nth(2)
            .map(|(idx, _)| idx + 1);

        let function = function_start.map(|start| &type_str[start..]);

        CotType {
            raw: type_str,
            atoms,
            affiliation,
            dimension,
            function,
        }
    }

    /// Fast check if this CoT type matches an affiliation
    #[inline]
    pub fn matches_affiliation(&self, target: Affiliation) -> bool {
        self.affiliation == Some(target)
    }

    /// Fast check if this CoT type is friendly
    #[inline]
    pub fn is_friendly(&self) -> bool {
        self.affiliation.is_some_and(|a| a.is_friendly())
    }

    /// Fast check if this CoT type is hostile
    #[inline]
    pub fn is_hostile(&self) -> bool {
        self.affiliation.is_some_and(|a| a.is_hostile())
    }

    /// Fast check if this CoT type is neutral
    #[inline]
    pub fn is_neutral(&self) -> bool {
        self.affiliation.is_some_and(|a| a.is_neutral())
    }

    /// Fast check if this CoT type matches a dimension
    #[inline]
    pub fn matches_dimension(&self, target: Dimension) -> bool {
        self.dimension == Some(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_friendly_ground() {
        let cot = CotType::parse("a-f-G-E-V-C-U-I");
        assert_eq!(cot.atoms, Some('a'));
        assert_eq!(cot.affiliation, Some(Affiliation::Friend));
        assert_eq!(cot.dimension, Some(Dimension::Ground));
        assert_eq!(cot.function, Some("E-V-C-U-I"));
        assert!(cot.is_friendly());
        assert!(!cot.is_hostile());
    }

    #[test]
    fn test_parse_hostile_air() {
        let cot = CotType::parse("a-h-A-M-F");
        assert_eq!(cot.affiliation, Some(Affiliation::Hostile));
        assert_eq!(cot.dimension, Some(Dimension::Air));
        assert!(cot.is_hostile());
        assert!(!cot.is_friendly());
    }

    #[test]
    fn test_parse_neutral() {
        let cot = CotType::parse("a-n-G");
        assert_eq!(cot.affiliation, Some(Affiliation::Neutral));
        assert!(cot.is_neutral());
        assert!(!cot.is_friendly());
        assert!(!cot.is_hostile());
    }

    #[test]
    fn test_zero_allocation() {
        let type_str = "a-f-G-E-V-C-U-I";
        let cot = CotType::parse(type_str);

        // Verify we're using the same string slice (zero-copy)
        assert_eq!(cot.raw.as_ptr(), type_str.as_ptr());
        if let Some(func) = cot.function {
            assert!(func.as_ptr() > type_str.as_ptr());
            assert!(func.as_ptr() < unsafe { type_str.as_ptr().add(type_str.len()) });
        }
    }

    #[test]
    fn test_affiliation_categories() {
        assert!(Affiliation::Friend.is_friendly());
        assert!(Affiliation::AssumedFriend.is_friendly());
        assert!(Affiliation::Joker.is_friendly());

        assert!(Affiliation::Hostile.is_hostile());
        assert!(Affiliation::Suspect.is_hostile());
        assert!(Affiliation::Faker.is_hostile());

        assert!(Affiliation::Neutral.is_neutral());
    }

    #[test]
    fn test_dimension_parsing() {
        assert_eq!(Dimension::from_char('G'), Some(Dimension::Ground));
        assert_eq!(Dimension::from_char('A'), Some(Dimension::Air));
        assert_eq!(Dimension::from_char('S'), Some(Dimension::SeaSurface));
        assert_eq!(Dimension::from_char('U'), Some(Dimension::SeaSubsurface));
        assert_eq!(Dimension::from_char('P'), Some(Dimension::Space));
        assert_eq!(Dimension::from_char('F'), Some(Dimension::SOF));
    }
}
