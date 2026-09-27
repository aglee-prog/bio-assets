PRAGMA journal_mode=WAL;
CREATE TABLE IF NOT EXISTS assets (
  rowid INTEGER PRIMARY KEY,
  id TEXT NOT NULL UNIQUE,
  source TEXT NOT NULL,
  upstream_key TEXT NOT NULL,
  name TEXT NOT NULL,
  category TEXT NOT NULL,
  tags TEXT NOT NULL,
  description TEXT NOT NULL,
  aliases TEXT NOT NULL,
  local_path TEXT NOT NULL,
  reusable_path TEXT NOT NULL,
  license TEXT NOT NULL,
  license_url TEXT,
  author TEXT,
  attribution TEXT NOT NULL,
  source_url TEXT NOT NULL,
  sha256 TEXT NOT NULL,
  source_revision TEXT,
  imported_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  UNIQUE(source, upstream_key)
);
CREATE INDEX IF NOT EXISTS assets_source_category ON assets(source, category);
CREATE VIRTUAL TABLE IF NOT EXISTS assets_fts USING fts5(
  name, category, tags, description, aliases,
  content='assets', content_rowid='rowid',
  tokenize='unicode61 remove_diacritics 2'
);
CREATE TRIGGER IF NOT EXISTS assets_ai AFTER INSERT ON assets BEGIN
  INSERT INTO assets_fts(rowid,name,category,tags,description,aliases)
  VALUES(new.rowid,new.name,new.category,new.tags,new.description,new.aliases);
END;
CREATE TRIGGER IF NOT EXISTS assets_ad AFTER DELETE ON assets BEGIN
  INSERT INTO assets_fts(assets_fts,rowid,name,category,tags,description,aliases)
  VALUES('delete',old.rowid,old.name,old.category,old.tags,old.description,old.aliases);
END;
CREATE TRIGGER IF NOT EXISTS assets_au AFTER UPDATE ON assets BEGIN
  INSERT INTO assets_fts(assets_fts,rowid,name,category,tags,description,aliases)
  VALUES('delete',old.rowid,old.name,old.category,old.tags,old.description,old.aliases);
  INSERT INTO assets_fts(rowid,name,category,tags,description,aliases)
  VALUES(new.rowid,new.name,new.category,new.tags,new.description,new.aliases);
END;
PRAGMA user_version=1;
