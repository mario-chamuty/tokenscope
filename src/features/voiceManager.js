const { ChannelType, PermissionFlagsBits } = require('discord.js');
const { db } = require('../database');
const { t } = require('../i18n');
const { getGuildConfig } = require('../utils/helpers');

async function handleVoiceStateUpdate(oldState, newState, client) {
  const guildId = newState.guild?.id || oldState.guild?.id;
  if (!guildId) return;

  const config = getGuildConfig(db, guildId);
  const lang = config.language || 'sk';
  const guild = newState.guild || oldState.guild;

  // User joined a channel
  if (newState.channelId && newState.channelId !== oldState.channelId) {
    // Check if joined the hub channel
    if (config.voice_hub_channel_id && newState.channelId === config.voice_hub_channel_id) {
      await createPersonalChannel(newState, guild, lang);
      return;
    }

    // Check if joined a monitored channel that is now full
    const monitored = db.prepare(
      'SELECT * FROM monitored_voice_channels WHERE channel_id = ? AND guild_id = ?'
    ).get(newState.channelId, guildId);

    if (monitored) {
      const channel = guild.channels.cache.get(newState.channelId);
      if (channel && channel.userLimit > 0 && channel.members.size >= channel.userLimit) {
        await cloneChannel(channel, guild, lang);
      }
    }
  }

  // User left a channel – check if temp channel should be deleted
  if (oldState.channelId && oldState.channelId !== newState.channelId) {
    const tempChannel = db.prepare(
      'SELECT * FROM temp_voice_channels WHERE channel_id = ?'
    ).get(oldState.channelId);

    if (tempChannel) {
      const channel = guild.channels.cache.get(oldState.channelId);
      if (channel && channel.members.size === 0) {
        await channel.delete('Temp voice channel empty').catch(() => {});
        db.prepare('DELETE FROM temp_voice_channels WHERE channel_id = ?').run(oldState.channelId);
      }
    }
  }
}

async function createPersonalChannel(voiceState, guild, lang) {
  const member = voiceState.member;
  const hubChannel = guild.channels.cache.get(voiceState.channelId);
  if (!hubChannel) return;

  const channelName = t('voice.personalChannel', lang, { user: member.displayName });

  const newChannel = await guild.channels.create({
    name: channelName,
    type: ChannelType.GuildVoice,
    parent: hubChannel.parent,
    userLimit: 10,
    permissionOverwrites: [
      {
        id: member.id,
        allow: [
          PermissionFlagsBits.ManageChannels,
          PermissionFlagsBits.MoveMembers,
          PermissionFlagsBits.MuteMembers,
        ],
      },
    ],
    reason: 'Personal temp voice channel',
  });

  db.prepare(
    "INSERT INTO temp_voice_channels (channel_id, guild_id, owner_id, type) VALUES (?, ?, ?, 'personal')"
  ).run(newChannel.id, guild.id, member.id);

  // Move user to the new channel
  await member.voice.setChannel(newChannel).catch(() => {});
}

async function cloneChannel(channel, guild, lang) {
  // Count existing clones
  const existingClones = db.prepare(
    "SELECT COUNT(*) as count FROM temp_voice_channels WHERE guild_id = ? AND source_channel_id = ? AND type = 'clone'"
  ).get(guild.id, channel.id);

  const cloneNumber = (existingClones?.count || 0) + 2;
  const channelName = t('voice.clonedChannel', lang, {
    channel: channel.name.replace(/ #\d+$/, ''),
    number: cloneNumber,
  });

  const newChannel = await guild.channels.create({
    name: channelName,
    type: ChannelType.GuildVoice,
    parent: channel.parent,
    userLimit: channel.userLimit,
    bitrate: channel.bitrate,
    permissionOverwrites: channel.permissionOverwrites.cache.map(po => ({
      id: po.id,
      allow: po.allow,
      deny: po.deny,
    })),
    reason: 'Auto-cloned voice channel (original full)',
  });

  db.prepare(
    "INSERT INTO temp_voice_channels (channel_id, guild_id, type, source_channel_id) VALUES (?, ?, 'clone', ?)"
  ).run(newChannel.id, guild.id, channel.id);
}

module.exports = { handleVoiceStateUpdate };
