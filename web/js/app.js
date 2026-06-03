// OmniTAK Web Interface - Main Application
// Handles TAK server connections, certificate management, and real-time monitoring

// Origin-relative so the UI works on any host/port (localhost, Docker, remote).
const API_BASE = `${window.location.origin}/api/v1`;

// --- Auth: persist JWT and attach it to every API request ---
const TOKEN_KEY = 'omnitak_token';
const getToken = () => localStorage.getItem(TOKEN_KEY);
const setToken = (t) => localStorage.setItem(TOKEN_KEY, t);
const clearToken = () => localStorage.removeItem(TOKEN_KEY);

// Wrap fetch so all /api/v1 calls carry the bearer token (except login itself),
// and a 401 bounces the user back to the login screen.
const _origFetch = window.fetch.bind(window);
window.fetch = (input, init = {}) => {
    const url = typeof input === 'string' ? input : (input && input.url) || '';
    const token = getToken();
    const isApi = url.includes('/api/v1/');
    const isLogin = url.endsWith('/auth/login');
    if (token && isApi && !isLogin) {
        init = { ...init, headers: { ...(init.headers || {}), Authorization: `Bearer ${token}` } };
    }
    return _origFetch(input, init).then((res) => {
        if (res.status === 401 && isApi && !isLogin) {
            clearToken();
            showLogin();
        }
        return res;
    });
};

async function doLogin(username, password) {
    const res = await _origFetch(`${API_BASE}/auth/login`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ username, password })
    });
    if (!res.ok) throw new Error('Invalid username or password');
    const data = await res.json();
    setToken(data.access_token);
    return data;
}

function showLogin() {
    const el = document.getElementById('login-overlay');
    if (el) el.style.display = 'flex';
}
function hideLogin() {
    const el = document.getElementById('login-overlay');
    if (el) el.style.display = 'none';
}

// Start the authenticated app (called once we hold a valid token).
function startApp() {
    hideLogin();
    checkSystemStatus();
    loadConnections();
    startStatusPolling();
}

function setupLoginForm() {
    const form = document.getElementById('login-form');
    if (form) {
        form.addEventListener('submit', async (e) => {
            e.preventDefault();
            const u = document.getElementById('login-username').value;
            const p = document.getElementById('login-password').value;
            const errEl = document.getElementById('login-error');
            errEl.textContent = '';
            try {
                await doLogin(u, p);
                startApp();
                showToast('Signed in', 'success');
            } catch (err) {
                errEl.textContent = err.message || 'Login failed';
            }
        });
    }
    const logoutBtn = document.getElementById('logout-btn');
    if (logoutBtn) {
        logoutBtn.addEventListener('click', () => {
            clearToken();
            location.reload();
        });
    }
}

// Application State
const state = {
    connections: [],
    stats: {
        messagesReceived: 0,
        messagesSent: 0,
        messagesFiltered: 0,
        messagesDuplicated: 0,
        throughput: 0,
        errors: 0
    },
    certificates: {
        clientCert: null,
        clientKey: null,
        caCert: null
    },
    systemStatus: 'offline'
};

// Initialize Application
document.addEventListener('DOMContentLoaded', () => {
    console.log('OmniTAK Web Interface Loaded');
    initializeEventListeners();
    setupLoginForm();
    // Gate the app behind login — show the dashboard only once authenticated.
    if (getToken()) {
        startApp();
    } else {
        showLogin();
    }
});

// Event Listeners
function initializeEventListeners() {
    // Protocol change handler
    const protocolSelect = document.getElementById('protocol');
    protocolSelect.addEventListener('change', handleProtocolChange);

    // File upload handlers
    setupFileUpload('client-cert', 'client-cert-name');
    setupFileUpload('client-key', 'client-key-name');
    setupFileUpload('ca-cert', 'ca-cert-name');

    // Form submission
    const form = document.getElementById('connection-form');
    form.addEventListener('submit', handleAddConnection);

    // Test connection button
    const testBtn = document.getElementById('test-connection');
    testBtn.addEventListener('click', handleTestConnection);

    // Clear messages button
    const clearMessagesBtn = document.getElementById('clear-messages');
    clearMessagesBtn.addEventListener('click', clearMessages);
}

// Protocol Selection Handler
function handleProtocolChange(event) {
    const tlsSection = document.getElementById('tls-section');
    if (event.target.value === 'tls') {
        tlsSection.style.display = 'block';
    } else {
        tlsSection.style.display = 'none';
    }
}

// File Upload Setup
function setupFileUpload(inputId, displayId) {
    const input = document.getElementById(inputId);
    const display = document.getElementById(displayId);

    input.addEventListener('change', (event) => {
        const file = event.target.files[0];
        if (file) {
            display.textContent = file.name;

            // Store file for upload
            const fileType = inputId.replace('-', '_');
            readFileAsBase64(file, (base64Data) => {
                state.certificates[fileType] = {
                    name: file.name,
                    data: base64Data,
                    size: file.size
                };
                showToast(`${file.name} loaded successfully`, 'success');
            });
        }
    });
}

// Read file as Base64
function readFileAsBase64(file, callback) {
    const reader = new FileReader();
    reader.onload = (e) => {
        const base64 = e.target.result.split(',')[1];
        callback(base64);
    };
    reader.readAsDataURL(file);
}

// Handle Add Connection
async function handleAddConnection(event) {
    event.preventDefault();

    const host = document.getElementById('server-host').value;
    const port = document.getElementById('server-port').value;
    const address = `${host}:${port}`;

    const protocol = document.getElementById('protocol').value;
    // Map the UI protocol to the server's ConnectionType (serde lowercase).
    const connectionTypeMap = { tcp: 'tcpclient', tls: 'tlsclient', udp: 'udp', multicast: 'multicast' };
    const verifyHostnameEl = document.getElementById('verify-hostname');

    // Shape matches the server's CreateConnectionRequest (host + port sent separately).
    const connectionData = {
        name: document.getElementById('connection-name').value,
        connection_type: connectionTypeMap[protocol] || 'tcpclient',
        address: host,
        port: parseInt(port),
        auto_reconnect: document.getElementById('auto-reconnect').checked,
        validate_certs: verifyHostnameEl ? verifyHostnameEl.checked : true
    };

    if (protocol === 'tls') {
        // Forward uploaded cert material (base64 PEM or PKCS#12) to the server.
        const certs = state.certificates;
        if (certs.client_cert && certs.client_cert.data) connectionData.tls_client_cert_pem_b64 = certs.client_cert.data;
        if (certs.client_key && certs.client_key.data) connectionData.tls_client_key_pem_b64 = certs.client_key.data;
        if (certs.ca_cert && certs.ca_cert.data) connectionData.tls_ca_cert_pem_b64 = certs.ca_cert.data;
        const certPassword = document.getElementById('cert-password').value;
        if (certPassword) connectionData.tls_cert_password = certPassword;
        if (!certs.client_cert || !certs.client_cert.data) {
            showToast('TLS selected but no client certificate uploaded', 'warning');
        }
    }

    try {
        // Send to backend API
        const response = await fetch(`${API_BASE}/connections`, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json'
            },
            body: JSON.stringify(connectionData)
        });

        if (response.ok) {
            await response.json();
            showToast(`Connection "${connectionData.name}" added successfully!`, 'success');

            // Refresh the list from the server so we render the canonical state.
            await loadConnections();

            // Reset form
            event.target.reset();
            clearCertificates();
        } else {
            const error = await response.json();
            throw new Error(error.message || 'Failed to add connection');
        }
    } catch (error) {
        console.error('Error adding connection:', error);
        showToast(`Error: ${error.message}`, 'error');
    }
}

// Handle Test Connection
async function handleTestConnection() {
    const host = document.getElementById('server-host').value;
    const port = document.getElementById('server-port').value;
    const protocol = document.getElementById('protocol').value;

    if (!host || !port) {
        showToast('Please enter server host and port', 'warning');
        return;
    }

    const address = `${host}:${port}`;

    showToast('Testing connection...', 'warning');

    try {
        const response = await fetch(`${API_BASE}/test-connection`, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json'
            },
            body: JSON.stringify({ address, protocol })
        });

        if (response.ok) {
            const result = await response.json();
            showToast(`Connection test successful! Latency: ${result.latency}ms`, 'success');
        } else {
            throw new Error('Connection test failed');
        }
    } catch (error) {
        console.error('Connection test error:', error);
        showToast('Connection test failed - server may be unreachable', 'error');
    }
}

// Clear Certificates
function clearCertificates() {
    state.certificates = {
        clientCert: null,
        clientKey: null,
        caCert: null
    };
    document.getElementById('client-cert-name').textContent = 'Choose file...';
    document.getElementById('client-key-name').textContent = 'Choose file...';
    document.getElementById('ca-cert-name').textContent = 'Choose file...';
}

// Render Connections
function renderConnections() {
    const connectionsList = document.getElementById('connections-list');

    if (state.connections.length === 0) {
        connectionsList.innerHTML = `
            <div class="empty-state">
                <p>No active connections. Add a TAK server above to get started.</p>
            </div>
        `;
        return;
    }

    connectionsList.innerHTML = state.connections.map(conn => {
        const statusClass = conn.status === 'connected' ? 'connected' :
                          conn.status === 'error' ? 'error' : '';
        const statusText = conn.status === 'connected' ? 'Connected' :
                          conn.status === 'connecting' ? 'Connecting...' :
                          conn.status === 'error' ? 'Error' : 'Disconnected';
        const statusBadgeClass = conn.status === 'connected' ? 'status-connected' :
                                conn.status === 'connecting' ? 'status-connecting' :
                                'status-disconnected';

        // Derive a short protocol label from the server's connection_type (e.g. tcpclient -> TCP).
        const proto = (conn.connection_type || 'tcpclient')
            .replace('client', '').replace('server', '').toUpperCase() || 'TCP';

        return `
            <div class="connection-item ${statusClass}" data-id="${conn.id}">
                <div class="connection-info">
                    <div class="connection-name">${conn.name}</div>
                    <div class="connection-details">
                        <span class="connection-badge badge-${proto.toLowerCase()}">${proto}</span>
                        <span><svg class="icon"><use href="#i-server"/></svg><span class="mono">${conn.address}:${conn.port}</span></span>
                        <span><svg class="icon"><use href="#i-download"/></svg>RX <span class="mono">${conn.messages_received || 0}</span></span>
                        <span><svg class="icon"><use href="#i-upload"/></svg>TX <span class="mono">${conn.messages_sent || 0}</span></span>
                    </div>
                </div>
                <div class="connection-status ${statusBadgeClass}">
                    ${statusText}
                </div>
                <div class="connection-actions">
                    <button class="btn btn-small btn-secondary" onclick="reconnectConnection('${conn.id}')">
                        <svg class="icon"><use href="#i-refresh"/></svg>Reconnect
                    </button>
                    <button class="btn btn-small btn-danger" onclick="removeConnection('${conn.id}')">
                        <svg class="icon"><use href="#i-trash"/></svg>Remove
                    </button>
                </div>
            </div>
        `;
    }).join('');
}

// Reconnect Connection
async function reconnectConnection(connectionId) {
    showToast('Reconnecting...', 'warning');

    try {
        const response = await fetch(`${API_BASE}/connections/${connectionId}/reconnect`, {
            method: 'POST'
        });

        if (response.ok) {
            showToast('Reconnected successfully', 'success');
            loadConnections();
        } else {
            throw new Error('Reconnection failed');
        }
    } catch (error) {
        console.error('Reconnection error:', error);
        showToast('Failed to reconnect', 'error');
    }
}

// Remove Connection
async function removeConnection(connectionId) {
    if (!confirm('Are you sure you want to remove this connection?')) {
        return;
    }

    try {
        const response = await fetch(`${API_BASE}/connections/${connectionId}`, {
            method: 'DELETE'
        });

        if (response.ok) {
            showToast('Connection removed', 'success');
            state.connections = state.connections.filter(c => c.id !== connectionId);
            renderConnections();
        } else {
            throw new Error('Failed to remove connection');
        }
    } catch (error) {
        console.error('Remove connection error:', error);
        showToast('Failed to remove connection', 'error');

        // Remove locally anyway
        state.connections = state.connections.filter(c => c.id !== connectionId);
        renderConnections();
    }
}

// Load Connections from API
async function loadConnections() {
    try {
        const response = await fetch(`${API_BASE}/connections`);
        if (response.ok) {
            const data = await response.json();
            // Server returns { connections: [...], total }
            state.connections = Array.isArray(data) ? data : (data.connections || []);
            renderConnections();
        }
    } catch (error) {
        console.log('Backend not available, using local state');
    }
}

// Check System Status
async function checkSystemStatus() {
    try {
        const response = await fetch(`${API_BASE}/status`);
        if (response.ok) {
            const status = await response.json();
            updateSystemStatus('online');
            updateStats(status);
        } else {
            updateSystemStatus('offline');
        }
    } catch (error) {
        updateSystemStatus('offline');
    }
}

// Update System Status
function updateSystemStatus(status) {
    state.systemStatus = status;
    const statusElement = document.getElementById('system-status');
    statusElement.textContent = status === 'online' ? 'Online' : 'Offline';
    statusElement.className = `status-value ${status}`;
}

// Update Statistics
function updateStats(stats) {
    // Server SystemStatus uses snake_case. Fields not yet exposed by the API
    // (sent/filtered/duplicated/errors) fall back to 0.
    const n = (v) => v ?? 0;
    document.getElementById('active-connections').textContent = n(stats.active_connections);
    document.getElementById('message-count').textContent = n(stats.messages_processed);
    document.getElementById('messages-received').textContent = n(stats.messages_processed);
    document.getElementById('messages-sent').textContent = n(stats.messages_sent);
    document.getElementById('messages-filtered').textContent = n(stats.messages_filtered);
    document.getElementById('messages-duplicated').textContent = n(stats.messages_duplicated);
    document.getElementById('throughput').textContent = `${n(stats.messages_per_second).toFixed(2)}/s`;
    document.getElementById('errors').textContent = n(stats.errors);
}

// Start Status Polling
function startStatusPolling() {
    setInterval(() => {
        checkSystemStatus();
        loadConnections();
    }, 5000); // Poll every 5 seconds
}

// Add Message to Log
function addMessageToLog(message) {
    const messagesLog = document.getElementById('messages-log');
    const autoScroll = document.getElementById('auto-scroll').checked;

    // Remove empty state if present
    const emptyState = messagesLog.querySelector('.empty-state');
    if (emptyState) {
        emptyState.remove();
    }

    const timestamp = new Date().toLocaleTimeString();
    const messageElement = document.createElement('div');
    messageElement.className = 'message-entry';
    messageElement.innerHTML = `
        <span class="message-time">[${timestamp}]</span>
        <span class="message-source">${message.source}</span>
        <span class="message-type">${message.type}</span>
        ${message.content}
    `;

    messagesLog.appendChild(messageElement);

    // Limit to last 100 messages
    while (messagesLog.children.length > 100) {
        messagesLog.removeChild(messagesLog.firstChild);
    }

    // Auto-scroll if enabled
    if (autoScroll) {
        messagesLog.scrollTop = messagesLog.scrollHeight;
    }
}

// Clear Messages
function clearMessages() {
    const messagesLog = document.getElementById('messages-log');
    messagesLog.innerHTML = `
        <div class="empty-state">
            <p>No messages yet. Messages will appear here when connections are active.</p>
        </div>
    `;
}

// Show Toast Notification
function showToast(message, type = 'success') {
    const toast = document.getElementById('toast');
    toast.textContent = message;
    toast.className = `toast ${type} show`;

    setTimeout(() => {
        toast.classList.remove('show');
    }, 3000);
}

// WebSocket Connection for Real-Time Updates
function connectWebSocket() {
    const wsProto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const ws = new WebSocket(`${wsProto}//${window.location.host}/api/v1/stream`);

    ws.onopen = () => {
        console.log('WebSocket connected');
        showToast('Real-time monitoring connected', 'success');
    };

    ws.onmessage = (event) => {
        try {
            const data = JSON.parse(event.data);

            if (data.type === 'message') {
                addMessageToLog(data.message);
            } else if (data.type === 'stats') {
                updateStats(data.stats);
            } else if (data.type === 'connection_update') {
                loadConnections();
            }
        } catch (error) {
            console.error('WebSocket message error:', error);
        }
    };

    ws.onerror = (error) => {
        console.error('WebSocket error:', error);
    };

    ws.onclose = () => {
        console.log('WebSocket disconnected, attempting to reconnect...');
        setTimeout(connectWebSocket, 5000);
    };
}

// Attempt WebSocket connection (disabled until backend WebSocket is implemented)
// setTimeout(connectWebSocket, 2000);

// Export functions to global scope for onclick handlers
window.reconnectConnection = reconnectConnection;
window.removeConnection = removeConnection;
