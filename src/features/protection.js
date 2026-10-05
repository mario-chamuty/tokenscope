const { PermissionFlagsBits } = require('discord.js');
const { db } = require('../database');
const { t } = require('../i18n');
const { createEmbed, getGuildConfig } = require('../utils/helpers');

// In-memory spam tracking (resets on bot restart)
const messageTracker = new Map(); // guildId:userId -> { messages: [], duplicates: 0 }

const SPAM_CONFIG = {
  maxMessages: 5,       // max messages in window
  windowMs: 5000,       // 5 seconds
  maxDuplicates: 3,     // max identical messages
  maxMentions: 5,       // max mentions per message
  muteDurationMs: 10 * 60 * 1000, // 10 minute mute
};

async function checkSpam(message) {
  if (!message.guild || message.author.bot) return false;
  if (message.member?.permissions.has(PermissionFlagsBits.ManageMessages)) return false;

  const key = `${message.guild.id}:${message.author.id}`;
  const config = getGuildConfig(db, message.guild.id);
  const lang = config.language || 'sk';

  if (!messageTracker.has(key)) {
    messageTracker.set(key, { messages: [], lastContent: '' });
  }

  const tracker = messageTracker.get(key);
  const now = Date.now();

  // Clean old messages from window
  tracker.messages = tracker.messages.filter(ts => now - ts < SPAM_CONFIG.windowMs);
  tracker.messages.push(now);

  // Check rate limit
  if (tracker.messages.length > SPAM_CONFIG.maxMessages) {
    await muteUser(message, lang, config);
    return true;
  }

  // Check duplicate messages
  if (message.content === tracker.lastContent) {
    tracker.duplicateCount = (tracker.duplicateCount || 0) + 1;
    if (tracker.duplicateCount >= SPAM_CONFIG.maxDuplicates) {
      await muteUser(message, lang, config);
      return true;
    }
  } else {
    tracker.duplicateCount = 0;
    tracker.lastContent = message.content;
  }

  // Check mention spam
  const mentionCount = message.mentions.users.size + message.mentions.roles.size;
  if (mentionCount > SPAM_CONFIG.maxMentions) {
    await muteUser(message, lang, config);
    return true;
  }

  return false;
}

async function muteUser(message, lang, config) {
  try {
    await message.member.timeout(SPAM_CONFIG.muteDurationMs, 'Anti-spam: automatic mute');
    await message.delete().catch(() => {});

    // Log to audit channel
    if (config.log_channel_id) {
      const logChannel = message.guild.channels.cache.get(config.log_channel_id);
      if (logChannel) {
        logChannel.send({
          embeds: [createEmbed({
            title: '🚫 Anti-spam',
            description: t('protection.spamDetected', lang, { user: message.author.tag }),
            color: 0xef4444,
          })],
        });
      }
    }

    messageTracker.delete(`${message.guild.id}:${message.author.id}`);
  } catch (error) {
    console.error('[AntiSpam] Failed to mute:', error.message);
  }
}

// Word filter
async function checkWordFilter(message) {
  if (!message.guild || message.author.bot) return false;
  if (message.member?.permissions.has(PermissionFlagsBits.ManageMessages)) return false;

  const config = getGuildConfig(db, message.guild.id);
  const lang = config.language || 'sk';

  const filteredWords = db.prepare(
    'SELECT word FROM filtered_words WHERE guild_id = ?'
  ).all(message.guild.id);

  if (filteredWords.length === 0) return false;

  const content = message.content.toLowerCase();
  const matched = filteredWords.some(fw => content.includes(fw.word.toLowerCase()));

  if (matched) {
    await message.delete().catch(() => {});

    if (config.log_channel_id) {
      const logChannel = message.guild.channels.cache.get(config.log_channel_id);
      if (logChannel) {
        logChannel.send({
          embeds: [createEmbed({
            title: '🚫 Word Filter',
            description: t('protection.wordFiltered', lang, { user: message.author.tag }),
            color: 0xf59e0b,
          })],
        });
      }
    }
    return true;
  }

  return false;
}

// Anti-nuke: track destructive actions per user
const NUKE_CONFIG = {
  maxActions: 5,     // max destructive actions
  windowMs: 10000,   // in 10 seconds
};

function sqliteNow() {
  // Use SQLite-compatible format to match datetime('now')
  return new Date().toISOString().replace('T', ' ').replace(/\.\d{3}Z$/, '');
}

async function trackNukeAction(guild, userId, actionType) {
  const now = sqliteNow();

  db.prepare(
    'INSERT INTO nuke_actions (guild_id, user_id, action_type, timestamp) VALUES (?, ?, ?, ?)'
  ).run(guild.id, userId, actionType, now);

  // Count recent actions
  const cutoff = new Date(Date.now() - NUKE_CONFIG.windowMs).toISOString().replace('T', ' ').replace(/\.\d{3}Z$/, '');
  const recentActions = db.prepare(
    'SELECT COUNT(*) as count FROM nuke_actions WHERE guild_id = ? AND user_id = ? AND timestamp > ?'
  ).get(guild.id, userId, cutoff);

  if (recentActions.count >= NUKE_CONFIG.maxActions) {
    await freezeUser(guild, userId);
  }
}

async function freezeUser(guild, userId) {
  const config = getGuildConfig(db, guild.id);
  const lang = config.language || 'sk';

  const member = await guild.members.fetch(userId).catch(() => null);
  if (!member) return;

  // Remove all dangerous permissions from the user's roles
  for (const [roleId, role] of member.roles.cache) {
    if (role.id === guild.id) continue; // skip @everyone
    if (role.permissions.has(PermissionFlagsBits.Administrator) ||
        role.permissions.has(PermissionFlagsBits.ManageChannels) ||
        role.permissions.has(PermissionFlagsBits.ManageRoles) ||
        role.permissions.has(PermissionFlagsBits.ManageGuild)) {
      try {
        await member.roles.remove(role, 'Anti-nuke: permissions frozen');
      } catch {
        // Bot may not have permission to remove this role
      }
    }
  }

  // Alert admin channel
  if (config.admin_channel_id) {
    const adminChannel = guild.channels.cache.get(config.admin_channel_id);
    if (adminChannel) {
      adminChannel.send({
        content: '@here',
        embeds: [createEmbed({
          title: '🚨 NUKE DETECTION',
          description: t('protection.nukeDetected', lang, { user: member.user.tag }),
          color: 0xdc2626,
          fields: [
            { name: 'User ID', value: userId, inline: true },
            { name: 'Action', value: 'Permissions frozen', inline: true },
          ],
        })],
      });
    }
  }
}

function startAntiNukeCleanup() {
  // Clean old nuke action records every 5 minutes
  setInterval(() => {
    const cutoff = new Date(Date.now() - 60000).toISOString().replace('T', ' ').replace(/\.\d{3}Z$/, '');
    db.prepare('DELETE FROM nuke_actions WHERE timestamp < ?').run(cutoff);
  }, 5 * 60 * 1000);
}

module.exports = { checkSpam, checkWordFilter, trackNukeAction, startAntiNukeCleanup };
