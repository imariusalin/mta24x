CREATE TABLE IF NOT EXISTS ips (
    id TEXT PRIMARY KEY,
    address TEXT NOT NULL UNIQUE,
    hostname TEXT NOT NULL,
    role TEXT NOT NULL,
    health TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    graduated_at TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS domains (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    stream TEXT NOT NULL,
    selector TEXT NOT NULL DEFAULT 'mail',
    dkim_private_pem TEXT NOT NULL,
    dkim_public TEXT NOT NULL,
    health TEXT NOT NULL DEFAULT 'warmup',
    first_sent_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS messages (
    id UUID PRIMARY KEY,
    stream TEXT NOT NULL,
    ip_id TEXT REFERENCES ips(id),
    domain_id TEXT REFERENCES domains(id),
    envelope_from TEXT NOT NULL,
    recipient TEXT NOT NULL,
    raw BYTEA NOT NULL,
    priority INT NOT NULL DEFAULT 50,
    status TEXT NOT NULL,
    attempts INT NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_error TEXT,
    smtp_code INT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    sent_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS messages_claim_idx
    ON messages (status, next_attempt_at, priority, created_at);

CREATE TABLE IF NOT EXISTS events (
    id BIGSERIAL PRIMARY KEY,
    message_id UUID REFERENCES messages(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    detail TEXT,
    smtp_code INT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS send_counters (
    ip_id TEXT NOT NULL,
    domain_id TEXT NOT NULL DEFAULT '',
    isp TEXT NOT NULL,
    bucket TIMESTAMPTZ NOT NULL,
    sent INT NOT NULL DEFAULT 0,
    bounced INT NOT NULL DEFAULT 0,
    complained INT NOT NULL DEFAULT 0,
    deferred INT NOT NULL DEFAULT 0,
    blocked INT NOT NULL DEFAULT 0,
    PRIMARY KEY (ip_id, domain_id, isp, bucket)
);

CREATE TABLE IF NOT EXISTS suppressions (
    email TEXT PRIMARY KEY,
    reason TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS api_keys (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    hash TEXT NOT NULL,
    prefix TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
