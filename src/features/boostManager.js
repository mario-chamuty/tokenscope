const { ActionRowBuilder, ButtonBuilder, ButtonStyle, PermissionFlagsBits, MessageFlags } = require('discord.js');
const { db } = require('../database');
const { t } = require('../i18n');
const { createEmbed, getGuildConfig } = require('../utils/helpers');

const CHECK_INTERVAL = 60 * 60 * 1000; // 1 hour

function startBoostChecker(client) {
  // Run immediately once, then every hour
  setTimeout(() => checkBoosts(client), 10_000);
  setInterval(() => checkBoosts(client), CHECK_INTERVAL);
  console.log('[BoostChecker] Started (hourly)');
}

async function checkBoosts(client) {
  for (const [guildId, guild] of client.guilds.cache) {
    const config = getGuildConfig(db, guildId);
    if (!config.boost_bot_role_id || !config.admin_channel_id) continue;

    const lang = config.language || 'sk';
    const adminChannel = guild.channels.cache.get(config.admin_channel_id);
    if (!adminChannel) continue;

    // Fetch all members with the boost bot role
    const boostBotRole = guild.roles.cache.get(config.boost_bot_role_id);
    if (!boostBotRole) continue;

    await guild.members.fetch();
    const boostBotMembers = boostBotRole.members;

    const expiredUsers = [];
    for (const [memberId, member] of boostBotMembers) {
      // Check if they are still boosting the server
      if (!member.premiumSince) {
        expiredUsers.push(member);
      }
    }

    if (expiredUsers.length > 0) {
      const userList = expiredUsers.map(m => `• ${m.user.tag} (${m.id})`).join('\n');

      const embed = createEmbed({
        title: t('boost.expiredTitle', lang),
        description: t('boost.expiredDesc', lang, { users: userList }),
        color: 0xef4444,
      });

      const row = new ActionRowBuilder().addComponents(
        new ButtonBuilder()
          .setCustomId(`boost_kickall_${Date.now()}`)
          .setLabel(`Kick All (${expiredUsers.length})`)
          .setStyle(ButtonStyle.Danger),
      );

      await adminChannel.send({ embeds: [embed], components: [row] });
    }
  }
}

async function lockdownBoostBots(guild) {
  const config = getGuildConfig(db, guild.id);
  if (!config.boost_bot_role_id) return;

  const boostBotRole = guild.roles.cache.get(config.boost_bot_role_id);
  if (!boostBotRole) return;

  // Ensure the role has no channel permissions (deny view on all channels)
  for (const [channelId, channel] of guild.channels.cache) {
    if (channel.isTextBased() || channel.isVoiceBased()) {
      await channel.permissionOverwrites.edit(boostBotRole, {
        ViewChannel: false,
      }).catch(() => {});
    }
  }
}

async function handleBoostKickInteraction(interaction) {
  if (!interaction.customId.startsWith('boost_kickall_')) return;

  // Only admins can use this
  if (!interaction.member.permissions.has(PermissionFlagsBits.Administrator)) {
    await interaction.reply({ content: '❌ Only admins can do this.', flags: MessageFlags.Ephemeral });
    return;
  }

  await interaction.deferReply();

  const config = getGuildConfig(db, interaction.guild.id);
  if (!config.boost_bot_role_id) {
    await interaction.editReply({ content: '❌ No boost bot role configured.' });
    return;
  }

  const boostBotRole = interaction.guild.roles.cache.get(config.boost_bot_role_id);
  if (!boostBotRole) {
    await interaction.editReply({ content: '❌ Boost bot role not found.' });
    return;
  }

  await interaction.guild.members.fetch();
  const expired = boostBotRole.members.filter(m => !m.premiumSince);

  let kicked = 0;
  let failed = 0;
  for (const [, member] of expired) {
    try {
      await member.kick('Boost expired – kicked via CraftingBot');
      kicked++;
    } catch {
      failed++;
    }
  }

  // Disable the button on the original message
  try {
    const disabledRow = new ActionRowBuilder().addComponents(
      new ButtonBuilder()
        .setCustomId('boost_kickall_done')
        .setLabel(`Kicked ${kicked}/${kicked + failed}`)
        .setStyle(ButtonStyle.Secondary)
        .setDisabled(true),
    );
    await interaction.message.edit({ components: [disabledRow] });
  } catch {
    // Message may have been deleted
  }

  await interaction.editReply({ content: `✅ Kicked **${kicked}** expired boost bots.${failed > 0 ? ` Failed: **${failed}**.` : ''}` });
}

module.exports = { startBoostChecker, lockdownBoostBots, handleBoostKickInteraction };
