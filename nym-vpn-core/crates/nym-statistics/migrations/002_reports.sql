CREATE TABLE pending_session_report (
    id                      INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    day                     DATE NOT NULL,
    connection_time_ms      INTEGER NOT NULL, 
    session_duration_min    INTEGER NOT NULL,
    two_hop                 BOOLEAN NOT NULL,
    exit_id                 TEXT NOT NULL,
    error                   TEXT
);