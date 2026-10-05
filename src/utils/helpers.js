const { EmbedBuilder } = require('discord.js');

function createEmbed({ title, description, color = 0x7c3aed, footer, thumbnail, fields, timestamp = true }) {
  const embed = new EmbedBuilder()
    .setColor(color);

  if (title) embed.setTitle(title);
  if (description) embed.setDescription(description);
  if (footer) embed.setFooter({ text: footer });
  if (thumbnail) embed.setThumbnail(thumbnail);
  if (fields) embed.addFields(fields);
  if (timestamp) embed.setTimestamp();

  return embed;
}

function getGuildConfig(db, guildId) {
  let config = db.prepare('SELECT * FROM guild_config WHERE guild_id = ?').get(guildId);
  if (!config) {
    db.prepare('INSERT OR IGNORE INTO guild_config (guild_id) VALUES (?)').run(guildId);
    config = db.prepare('SELECT * FROM guild_config WHERE guild_id = ?').get(guildId);
  }
  return config;
}

function updateGuildConfig(db, guildId, key, value) {
  const allowed = [
    'language', 'welcome_channel_id', 'leave_channel_id', 'log_channel_id',
    'admin_channel_id', 'selfrole_channel_id', 'voice_hub_channel_id',
    'boost_bot_role_id', 'full_member_role_id', 'temp_member_role_id',
    'auto_role_id',
    'stats_category_id', 'stats_members_channel_id',
    'stats_online_channel_id', 'stats_boosts_channel_id',
  ];
  if (!allowed.includes(key)) return false;
  getGuildConfig(db, guildId); // ensure row exists
  db.prepare(`UPDATE guild_config SET ${key} = ? WHERE guild_id = ?`).run(value, guildId);
  return true;
}

module.exports = { createEmbed, getGuildConfig, updateGuildConfig };
