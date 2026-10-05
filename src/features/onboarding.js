const { ActionRowBuilder, ButtonBuilder, ButtonStyle, StringSelectMenuBuilder } = require('discord.js');
const { db } = require('../database');
const { t } = require('../i18n');
const { createEmbed, getGuildConfig } = require('../utils/helpers');

async function startOnboarding(member, client) {
  const config = getGuildConfig(db, member.guild.id);
  const lang = config.language || 'sk';

  // Get available game roles for this guild
  const gameRoles = db.prepare(
    "SELECT * FROM self_roles WHERE guild_id = ? AND category = 'game'"
  ).all(member.guild.id);

  // Save onboarding state
  db.prepare(
    'INSERT OR REPLACE INTO onboarding_state (guild_id, user_id, step, data) VALUES (?, ?, ?, ?)'
  ).run(member.guild.id, member.id, 'games', '{}');

  try {
    const dm = await member.createDM();

    const welcomeEmbed = createEmbed({
      title: t('onboarding.welcome', lang, { user: member.displayName, server: member.guild.name }),
      description: t('onboarding.welcomeDesc', lang),
      color: 0x7c3aed,
      thumbnail: member.guild.iconURL({ dynamic: true }),
    });

    await dm.send({ embeds: [welcomeEmbed] });

    // Step 1: Games
    if (gameRoles.length > 0) {
      const gameOptions = gameRoles.slice(0, 25).map(role => ({
        label: role.label,
        value: role.role_id,
        emoji: role.emoji || undefined,
      }));

      const gameSelect = new StringSelectMenuBuilder()
        .setCustomId(`onboard_games_${member.guild.id}`)
        .setPlaceholder(t('onboarding.askGames', lang))
        .setMinValues(0)
        .setMaxValues(gameOptions.length)
        .addOptions(gameOptions);

      await dm.send({
        content: `**🎮 ${t('onboarding.askGames', lang)}**`,
        components: [new ActionRowBuilder().addComponents(gameSelect)],
      });
    } else {
      // No game roles, skip to interests
      await askInterests(member, lang, config);
    }
  } catch (error) {
    // DMs disabled
    if (config.welcome_channel_id) {
      const channel = member.guild.channels.cache.get(config.welcome_channel_id);
      if (channel) {
        channel.send(t('onboarding.dmDisabled', lang, { user: member.toString() }));
      }
    }
  }
}

async function askInterests(member, lang, config) {
  const interestRoles = db.prepare(
    "SELECT * FROM self_roles WHERE guild_id = ? AND category = 'interest'"
  ).all(member.guild.id);

  if (interestRoles.length === 0) {
    // No interest roles, skip to member type
    await askMemberType(member, lang, config);
    return;
  }

  try {
    const dm = await member.createDM();

    const interestOptions = interestRoles.slice(0, 25).map(role => ({
      label: role.label,
      value: role.role_id,
      emoji: role.emoji || undefined,
    }));

    const interestSelect = new StringSelectMenuBuilder()
      .setCustomId(`onboard_interests_${member.guild.id}`)
      .setPlaceholder(t('onboarding.askInterests', lang))
      .setMinValues(0)
      .setMaxValues(interestOptions.length)
      .addOptions(interestOptions);

    await dm.send({
      content: `**💡 ${t('onboarding.askInterests', lang)}**`,
      components: [new ActionRowBuilder().addComponents(interestSelect)],
    });
  } catch {
    // DMs disabled
  }
}

async function askMemberType(member, lang, config) {
  try {
    const dm = await member.createDM();

    const row = new ActionRowBuilder().addComponents(
      new ButtonBuilder()
        .setCustomId(`onboard_member_full_${member.guild.id}`)
        .setLabel(t('onboarding.memberTypeFull', lang))
        .setStyle(ButtonStyle.Primary),
      new ButtonBuilder()
        .setCustomId(`onboard_member_temp_${member.guild.id}`)
        .setLabel(t('onboarding.memberTypeTemp', lang))
        .setStyle(ButtonStyle.Secondary),
    );

    await dm.send({
      content: `**👤 ${t('onboarding.askMemberType', lang)}**`,
      components: [row],
    });
  } catch {
    // DMs disabled
  }
}

async function handleOnboardingInteraction(interaction, client) {
  const customId = interaction.customId;

  const parts = customId.split('_');
  const guildId = parts[parts.length - 1];
  const guild = client.guilds.cache.get(guildId);
  if (!guild) return;

  const config = getGuildConfig(db, guildId);
  const lang = config.language || 'sk';
  const member = await guild.members.fetch(interaction.user.id).catch(() => null);
  if (!member) return;

  // Handle game selection
  if (customId.startsWith('onboard_games_')) {
    const selectedRoleIds = interaction.values || [];

    for (const roleId of selectedRoleIds) {
      const role = guild.roles.cache.get(roleId);
      if (role) await member.roles.add(role).catch(() => {});
    }

    db.prepare(
      'UPDATE onboarding_state SET step = ?, data = ? WHERE guild_id = ? AND user_id = ?'
    ).run('interests', JSON.stringify({ games: selectedRoleIds }), guildId, interaction.user.id);

    await interaction.update({ components: [] });
    await askInterests(member, lang, config);
    return;
  }

  // Handle interest selection
  if (customId.startsWith('onboard_interests_')) {
    const selectedRoleIds = interaction.values || [];

    for (const roleId of selectedRoleIds) {
      const role = guild.roles.cache.get(roleId);
      if (role) await member.roles.add(role).catch(() => {});
    }

    db.prepare(
      'UPDATE onboarding_state SET step = ? WHERE guild_id = ? AND user_id = ?'
    ).run('member_type', guildId, interaction.user.id);

    await interaction.update({ components: [] });
    await askMemberType(member, lang, config);
    return;
  }

  // Handle member type
  if (customId.startsWith('onboard_member_')) {
    const isFull = customId.includes('_full_');

    if (isFull && config.full_member_role_id) {
      const role = guild.roles.cache.get(config.full_member_role_id);
      if (role) await member.roles.add(role).catch(() => {});
    } else if (!isFull && config.temp_member_role_id) {
      const role = guild.roles.cache.get(config.temp_member_role_id);
      if (role) await member.roles.add(role).catch(() => {});
    }

    // Onboarding complete
    db.prepare('DELETE FROM onboarding_state WHERE guild_id = ? AND user_id = ?')
      .run(guildId, interaction.user.id);

    await interaction.update({
      content: t('onboarding.onboardingComplete', lang),
      components: [],
    });
  }
}

module.exports = { startOnboarding, handleOnboardingInteraction };
