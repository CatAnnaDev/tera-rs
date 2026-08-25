use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

pub struct Database {
    connection: Connection,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS accounts (
    id      INTEGER PRIMARY KEY,
    name    TEXT NOT NULL UNIQUE,
    created INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE TABLE IF NOT EXISTS characters (
    id          INTEGER PRIMARY KEY,
    account     INTEGER NOT NULL REFERENCES accounts(id),
    name        TEXT NOT NULL UNIQUE COLLATE NOCASE,
    gender      INTEGER NOT NULL,
    race        INTEGER NOT NULL,
    class       INTEGER NOT NULL,
    level       INTEGER NOT NULL DEFAULT 1,
    xp          INTEGER NOT NULL DEFAULT 0,
    hp          INTEGER NOT NULL DEFAULT -1,
    gold        INTEGER NOT NULL DEFAULT 0,
    appearance  INTEGER NOT NULL DEFAULT 0,
    appearance2 INTEGER NOT NULL DEFAULT 0,
    details     BLOB NOT NULL DEFAULT x'',
    shape       BLOB NOT NULL DEFAULT x'',
    slot        INTEGER NOT NULL DEFAULT 1,
    walk_speed  INTEGER NOT NULL DEFAULT 50,
    run_speed   INTEGER NOT NULL DEFAULT 150,
    admin_level INTEGER NOT NULL DEFAULT 0,
    zone        INTEGER NOT NULL DEFAULT 0,
    x           REAL NOT NULL DEFAULT 0,
    y           REAL NOT NULL DEFAULT 0,
    z           REAL NOT NULL DEFAULT 0,
    w           INTEGER NOT NULL DEFAULT 0,
    created     INTEGER NOT NULL DEFAULT (unixepoch()),
    last_login  INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS equipment (
    character INTEGER NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    slot      INTEGER NOT NULL,
    item      INTEGER NOT NULL,
    PRIMARY KEY (character, slot)
);
CREATE TABLE IF NOT EXISTS inventory (
    character INTEGER NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    slot      INTEGER NOT NULL,
    item      INTEGER NOT NULL,
    amount    INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (character, slot)
);
CREATE TABLE IF NOT EXISTS learned_skills (
    character INTEGER NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    skill     INTEGER NOT NULL,
    PRIMARY KEY (character, skill)
);
CREATE INDEX IF NOT EXISTS characters_by_account ON characters(account);
";

const COLUMNS: [(&str, &str); 1] = [("hp", "INTEGER NOT NULL DEFAULT -1")];

fn migrate(connection: &Connection) {
    for (name, kind) in COLUMNS {
        let _ = connection.execute(
            &format!("ALTER TABLE characters ADD COLUMN {name} {kind}"),
            [],
        );
    }
}

impl Database {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.execute_batch(SCHEMA)?;
        migrate(&connection);
        Ok(Self { connection })
    }

    pub fn memory() -> rusqlite::Result<Self> {
        let connection = Connection::open_in_memory()?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.execute_batch(SCHEMA)?;
        migrate(&connection);
        Ok(Self { connection })
    }

    pub fn account(&self, name: &str) -> rusqlite::Result<i64> {
        self.connection.execute(
            "INSERT OR IGNORE INTO accounts (name) VALUES (?1)",
            params![name],
        )?;
        self.connection.query_row(
            "SELECT id FROM accounts WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )
    }

    pub fn character_count(&self, account: i64) -> rusqlite::Result<i64> {
        self.connection.query_row(
            "SELECT count(*) FROM characters WHERE account = ?1",
            params![account],
            |row| row.get(0),
        )
    }

    pub fn name_taken(&self, name: &str) -> rusqlite::Result<bool> {
        let found: Option<i64> = self
            .connection
            .query_row(
                "SELECT id FROM characters WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_schema_applies_to_a_fresh_database() {
        let db = Database::memory().expect("open");
        let account = db.account("35171").expect("account");
        assert_eq!(db.character_count(account).expect("count"), 0);
    }

    #[test]
    fn an_account_is_created_once() {
        let db = Database::memory().expect("open");
        let first = db.account("35171").expect("account");
        let second = db.account("35171").expect("account");
        assert_eq!(first, second);
    }

    #[test]
    fn names_are_compared_without_case() {
        let db = Database::memory().expect("open");
        let account = db.account("a").expect("account");
        db.connection()
            .execute(
                "INSERT INTO characters (account, name, gender, race, class) VALUES (?1,'Meow',1,4,6)",
                params![account],
            )
            .expect("insert");
        assert!(db.name_taken("meow").expect("taken"));
        assert!(!db.name_taken("other").expect("taken"));
    }
}
