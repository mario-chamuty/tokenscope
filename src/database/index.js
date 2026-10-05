const Database = require('better-sqlite3');
const path = require('path');
const fs = require('fs');

const dataDir = path.join(__dirname, '..', '..', 'data');
if (!fs.existsSync(dataDir)) fs.mkdirSync(dataDir, { recursive: true });

const db = new Database(path.join(dataDir, 'craftingbot.db'));

// Enable WAL mode for better performance
db.pragma('journal_mode = WAL');
db.pragma('foreign_keys = ON');

function initialize() {
  db.exec(`
    -- Server configuration
    CREATE TABLE IF NOT EXISTS guild_config (
      guild_id TEXT PRIMARY KEY,
      language TEXT DEFAULT 'sk',
      welcome_channel_id TEXT,
      leave_channel_id TEXT,
      log_channel_id TEXT,
      admin_channel_id TEXT,
      selfrole_channel_id TEXT,
      voice_hub_channel_id TEXT,
      boost_bot_role_id TEXT,
      full_member_role_id TEXT,
      temp_member_role_id TEXT,
      auto_role_id TEXT,
      stats_category_id TEXT,
      stats_members_channel_id TEXT,
      stats_online_channel_id TEXT,
      stats_boosts_channel_id TEXT
    );

    -- Self-assignable roles
    CREATE TABLE IF NOT EXISTS self_roles (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      guild_id TEXT NOT NULL,
      role_id TEXT NOT NULL,
      category TEXT NOT NULL DEFAULT 'game',
      label TEXT NOT NULL,
      emoji TEXT,
      UNIQUE(guild_id, role_id)
    );

    -- Role suggestions from users
    CREATE TABLE IF NOT EXISTS role_suggestions (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      guild_id TEXT NOT NULL,
      user_id TEXT NOT NULL,
      game_name TEXT NOT NULL,
      status TEXT DEFAULT 'pending',
      created_at TEXT DEFAULT (datetime('now'))
    );

    -- Temporary voice channels
    CREATE TABLE IF NOT EXISTS temp_voice_channels (
      channel_id TEXT PRIMARY KEY,
      guild_id TEXT NOT NULL,
      owner_id TEXT,
      type TEXT NOT NULL DEFAULT 'personal',
      source_channel_id TEXT,
      created_at TEXT DEFAULT (datetime('now'))
    );

    -- Voice channels to monitor for auto-cloning
    CREATE TABLE IF NOT EXISTS monitored_voice_channels (
      channel_id TEXT PRIMARY KEY,
      guild_id TEXT NOT NULL
    );

    -- Word filter
    CREATE TABLE IF NOT EXISTS filtered_words (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      guild_id TEXT NOT NULL,
      word TEXT NOT NULL,
      UNIQUE(guild_id, word)
    );

    -- Anti-spam tracking (in-memory would be better but we persist for restart resilience)
    CREATE TABLE IF NOT EXISTS spam_mutes (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      guild_id TEXT NOT NULL,
      user_id TEXT NOT NULL,
      muted_at TEXT DEFAULT (datetime('now')),
      expires_at TEXT
    );

    -- Onboarding state tracking
    CREATE TABLE IF NOT EXISTS onboarding_state (
      guild_id TEXT NOT NULL,
      user_id TEXT NOT NULL,
      step TEXT DEFAULT 'games',
      data TEXT DEFAULT '{}',
      started_at TEXT DEFAULT (datetime('now')),
      PRIMARY KEY (guild_id, user_id)
    );

    -- Anti-nuke action tracking
    CREATE TABLE IF NOT EXISTS nuke_actions (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      guild_id TEXT NOT NULL,
      user_id TEXT NOT NULL,
      action_type TEXT NOT NULL,
      timestamp TEXT DEFAULT (datetime('now'))
    );
  `);

  // Migrations – add columns that may not exist yet
  const migrations = [
    'ALTER TABLE guild_config ADD COLUMN auto_role_id TEXT',
    'ALTER TABLE guild_config ADD COLUMN stats_category_id TEXT',
    'ALTER TABLE guild_config ADD COLUMN stats_members_channel_id TEXT',
    'ALTER TABLE guild_config ADD COLUMN stats_online_channel_id TEXT',
    'ALTER TABLE guild_config ADD COLUMN stats_boosts_channel_id TEXT',
  ];

  for (const sql of migrations) {
    try {
      db.exec(sql);
    } catch {
      // Column already exists, ignore
    }
  }
}

module.exports = { db, initialize };
