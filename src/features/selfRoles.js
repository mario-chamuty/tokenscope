const {
  ActionRowBuilder, ButtonBuilder, ButtonStyle, StringSelectMenuBuilder,
  ModalBuilder, TextInputBuilder, TextInputStyle, MessageFlags,
} = require('discord.js');
const { db } = require('../database');
const { t } = require('../i18n');
const { createEmbed, getGuildConfig } = require('../utils/helpers');

async function sendSelfRoleEmbeds(channel, guildId) {
  const config = getGuildConfig(db, guildId);
  const lang = config.language || 'sk';

  const categories = [
    { key: 'game', titleKey: 'selfroles.gameRolesTitle', color: 0x7c3aed },
    { key: 'interest', titleKey: 'selfroles.interestRolesTitle', color: 0xec4899 },
    { key: 'notification', titleKey: 'selfroles.notificationRolesTitle', color: 0x10b981 },
  ];

  for (const cat of categories) {
    const roles = db.prepare(
      'SELECT * FROM self_roles WHERE guild_id = ? AND category = ?'
    ).all(guildId, cat.key);

    if (roles.length === 0) continue;

    const embed = createEmbed({
      title: t(cat.titleKey, lang),
      description: t('selfroles.description', lang),
      color: cat.color,
    });

    // Create buttons in rows of 5
    const rows = [];
    for (let i = 0; i < roles.length; i += 5) {
      const row = new ActionRowBuilder();
      const chunk = roles.slice(i, i + 5);
      for (const role of chunk) {
        const btn = new ButtonBuilder()
          .setCustomId(`selfrole_${role.role_id}`)
          .setLabel(role.label)
          .setStyle(ButtonStyle.Secondary);
        if (role.emoji) btn.setEmoji(role.emoji);
        row.addComponents(btn);
      }
      rows.push(row);
    }

    // Add suggest button for game category
    if (cat.key === 'game') {
      const suggestRow = new ActionRowBuilder().addComponents(
        new ButtonBuilder()
          .setCustomId('selfrole_suggest_game')
          .setLabel('💡 Suggest a game')
          .setStyle(ButtonStyle.Success),
      );
      rows.push(suggestRow);
    }

    await channel.send({ embeds: [embed], components: rows.slice(0, 5) }); // Discord max 5 rows
  }
}

async function handleSelfRoleInteraction(interaction, client) {
  const customId = interaction.customId;

  // Handle game suggestion button
  if (customId === 'selfrole_suggest_game') {
    const modal = new ModalBuilder()
      .setCustomId('selfrole_suggest_modal')
      .setTitle('Suggest a Game / Navrhni hru');

    const gameInput = new TextInputBuilder()
      .setCustomId('game_name')
      .setLabel('Game name / Názov hry')
      .setStyle(TextInputStyle.Short)
      .setMaxLength(50)
      .setRequired(true);

    modal.addComponents(new ActionRowBuilder().addComponents(gameInput));
    await interaction.showModal(modal);
    return;
  }

  // Handle suggestion modal submit
  if (customId === 'selfrole_suggest_modal') {
    const gameName = interaction.fields.getTextInputValue('game_name');
    const config = getGuildConfig(db, interaction.guild.id);
    const lang = config.language || 'sk';

    db.prepare(
      'INSERT INTO role_suggestions (guild_id, user_id, game_name) VALUES (?, ?, ?)'
    ).run(interaction.guild.id, interaction.user.id, gameName);

    // Notify admin channel
    if (config.admin_channel_id) {
      const adminChannel = interaction.guild.channels.cache.get(config.admin_channel_id);
      if (adminChannel) {
        const approveRow = new ActionRowBuilder().addComponents(
          new ButtonBuilder()
            .setCustomId(`suggestion_approve_${gameName}`)
            .setLabel('Approve')
            .setStyle(ButtonStyle.Success),
          new ButtonBuilder()
            .setCustomId(`suggestion_deny_${gameName}`)
            .setLabel('Deny')
            .setStyle(ButtonStyle.Danger),
        );

        await adminChannel.send({
          embeds: [createEmbed({
            title: '💡 New Game Suggestion',
            description: `**${interaction.user.tag}** suggests adding: **${gameName}**`,
            color: 0xf59e0b,
          })],
          components: [approveRow],
        });
      }
    }

    await interaction.reply({
      content: t('selfroles.suggestionSent', lang, { game: gameName }),
      flags: MessageFlags.Ephemeral,
    });
    return;
  }

  // Handle suggestion approval/denial
  if (customId.startsWith('suggestion_approve_') || customId.startsWith('suggestion_deny_')) {
    if (!interaction.member.permissions.has('ManageRoles')) {
      await interaction.reply({ content: '❌ No permission.', flags: MessageFlags.Ephemeral });
      return;
    }

    const isApprove = customId.startsWith('suggestion_approve_');
    const gameName = isApprove
      ? customId.slice('suggestion_approve_'.length)
      : customId.slice('suggestion_deny_'.length);
    const config = getGuildConfig(db, interaction.guild.id);
    const lang = config.language || 'sk';

    if (isApprove) {
      // Create the role
      const role = await interaction.guild.roles.create({
        name: gameName,
        reason: 'Game role suggested and approved',
      });

      // Add to self_roles
      db.prepare(
        "INSERT OR IGNORE INTO self_roles (guild_id, role_id, category, label) VALUES (?, ?, 'game', ?)"
      ).run(interaction.guild.id, role.id, gameName);

      db.prepare(
        "UPDATE role_suggestions SET status = 'approved' WHERE guild_id = ? AND game_name = ? AND status = 'pending'"
      ).run(interaction.guild.id, gameName);

      await interaction.update({
        content: t('selfroles.suggestionApproved', lang, { game: gameName }),
        components: [],
      });

      // Refresh self-role channel
      if (config.selfrole_channel_id) {
        const channel = interaction.guild.channels.cache.get(config.selfrole_channel_id);
        if (channel) {
          // Delete old messages and resend
          const messages = await channel.messages.fetch({ limit: 50 });
          const botMessages = messages.filter(m => m.author.id === interaction.client.user.id);
          await Promise.all(botMessages.map(m => m.delete().catch(() => {})));
          await sendSelfRoleEmbeds(channel, interaction.guild.id);
        }
      }
    } else {
      db.prepare(
        "UPDATE role_suggestions SET status = 'denied' WHERE guild_id = ? AND game_name = ? AND status = 'pending'"
      ).run(interaction.guild.id, gameName);

      await interaction.update({
        content: t('selfroles.suggestionDenied', lang, { game: gameName }),
        components: [],
      });
    }
    return;
  }

  // Handle role toggle button
  if (customId.startsWith('selfrole_')) {
    const roleId = customId.replace('selfrole_', '');
    const config = getGuildConfig(db, interaction.guild.id);
    const lang = config.language || 'sk';
    const role = interaction.guild.roles.cache.get(roleId);

    if (!role) {
      await interaction.reply({ content: 'Role not found.', flags: MessageFlags.Ephemeral });
      return;
    }

    const member = interaction.member;
    try {
      if (member.roles.cache.has(roleId)) {
        await member.roles.remove(role);
        await interaction.reply({
          content: t('selfroles.roleRemoved', lang, { role: role.name }),
          flags: MessageFlags.Ephemeral,
        });
      } else {
        await member.roles.add(role);
        await interaction.reply({
          content: t('selfroles.roleAdded', lang, { role: role.name }),
          flags: MessageFlags.Ephemeral,
        });
      }
    } catch {
      await interaction.reply({
        content: '❌ Could not update role. The bot may lack permissions.',
        flags: MessageFlags.Ephemeral,
      }).catch(() => {});
    }
  }
}

module.exports = { sendSelfRoleEmbeds, handleSelfRoleInteraction };
