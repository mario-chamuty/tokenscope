const { ChannelType, PermissionFlagsBits } = require('discord.js');
const { db } = require('../database');
const { getGuildConfig, updateGuildConfig } = require('../utils/helpers');

const UPDATE_INTERVAL = 5 * 60 * 1000; // 5 minutes

function startStatsUpdater(client) {
  // Initial update after 15 seconds (let cache populate)
  setTimeout(() => updateAllStats(client), 15_000);
  setInterval(() => updateAllStats(client), UPDATE_INTERVAL);
  console.log('[ServerStats] Started (every 5 min)');
}

async function updateAllStats(client) {
  for (const [guildId, guild] of client.guilds.cache) {
    const config = getGuildConfig(db, guildId);
    if (!config.stats_category_id) continue;

    try {
      await updateGuildStats(guild, config);
    } catch (err) {
      console.error(`[ServerStats] Error updating ${guild.name}:`, err.message);
    }
  }
}

async function updateGuildStats(guild, config) {
  // Fetch all members to get accurate online count
  await guild.members.fetch({ withPresences: true }).catch(() => {});

  const totalMembers = guild.memberCount;
  const onlineMembers = guild.members.cache.filter(
    m => m.presence && m.presence.status !== 'offline'
  ).size;
  const boostCount = guild.premiumSubscriptionCount || 0;

  // Update member count channel
  if (config.stats_members_channel_id) {
    const channel = guild.channels.cache.get(config.stats_members_channel_id);
    if (channel) {
      const name = `👥 Members: ${totalMembers}`;
      if (channel.name !== name) {
        await channel.setName(name).catch(() => {});
      }
    }
  }

  // Update online count channel
  if (config.stats_online_channel_id) {
    const channel = guild.channels.cache.get(config.stats_online_channel_id);
    if (channel) {
      const name = `🟢 Online: ${onlineMembers}`;
      if (channel.name !== name) {
        await channel.setName(name).catch(() => {});
      }
    }
  }

  // Update boost count channel
  if (config.stats_boosts_channel_id) {
    const channel = guild.channels.cache.get(config.stats_boosts_channel_id);
    if (channel) {
      const name = `🚀 Boosts: ${boostCount}`;
      if (channel.name !== name) {
        await channel.setName(name).catch(() => {});
      }
    }
  }
}

async function createStatsChannels(guild) {
  const config = getGuildConfig(db, guild.id);

  // Create category
  const category = await guild.channels.create({
    name: '📊 Server Stats',
    type: ChannelType.GuildCategory,
    permissionOverwrites: [
      {
        id: guild.id,
        deny: [PermissionFlagsBits.Connect],
      },
    ],
    reason: 'Server stats channels',
  });

  // Create stat voice channels (voice channels show in sidebar nicely)
  const membersChannel = await guild.channels.create({
    name: `👥 Members: ${guild.memberCount}`,
    type: ChannelType.GuildVoice,
    parent: category,
    permissionOverwrites: [
      {
        id: guild.id,
        deny: [PermissionFlagsBits.Connect],
      },
    ],
  });

  const onlineChannel = await guild.channels.create({
    name: '🟢 Online: 0',
    type: ChannelType.GuildVoice,
    parent: category,
    permissionOverwrites: [
      {
        id: guild.id,
        deny: [PermissionFlagsBits.Connect],
      },
    ],
  });

  const boostsChannel = await guild.channels.create({
    name: `🚀 Boosts: ${guild.premiumSubscriptionCount || 0}`,
    type: ChannelType.GuildVoice,
    parent: category,
    permissionOverwrites: [
      {
        id: guild.id,
        deny: [PermissionFlagsBits.Connect],
      },
    ],
  });

  // Save to config
  updateGuildConfig(db, guild.id, 'stats_category_id', category.id);
  updateGuildConfig(db, guild.id, 'stats_members_channel_id', membersChannel.id);
  updateGuildConfig(db, guild.id, 'stats_online_channel_id', onlineChannel.id);
  updateGuildConfig(db, guild.id, 'stats_boosts_channel_id', boostsChannel.id);

  // Immediately update with real values
  await updateGuildStats(guild, getGuildConfig(db, guild.id));

  return category;
}

async function removeStatsChannels(guild) {
  const config = getGuildConfig(db, guild.id);

  const channelIds = [
    config.stats_members_channel_id,
    config.stats_online_channel_id,
    config.stats_boosts_channel_id,
    config.stats_category_id,
  ];

  for (const id of channelIds) {
    if (id) {
      const channel = guild.channels.cache.get(id);
      if (channel) await channel.delete('Server stats removed').catch(() => {});
    }
  }

  updateGuildConfig(db, guild.id, 'stats_category_id', null);
  updateGuildConfig(db, guild.id, 'stats_members_channel_id', null);
  updateGuildConfig(db, guild.id, 'stats_online_channel_id', null);
  updateGuildConfig(db, guild.id, 'stats_boosts_channel_id', null);
}

module.exports = { startStatsUpdater, createStatsChannels, removeStatsChannels };
