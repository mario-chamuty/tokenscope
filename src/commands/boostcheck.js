const { SlashCommandBuilder, PermissionFlagsBits, MessageFlags } = require('discord.js');
const { db } = require('../database');
const { t } = require('../i18n');
const { createEmbed, getGuildConfig } = require('../utils/helpers');

module.exports = {
  data: new SlashCommandBuilder()
    .setName('boostcheck')
    .setDescription('Manually check boost bot status')
    .setDefaultMemberPermissions(PermissionFlagsBits.Administrator),

  async execute(interaction) {
    const config = getGuildConfig(db, interaction.guild.id);
    const lang = config.language || 'sk';

    if (!config.boost_bot_role_id) {
      await interaction.reply({ content: 'Boost bot role not configured. Use `/setup boostrole` first.', flags: MessageFlags.Ephemeral });
      return;
    }

    const boostBotRole = interaction.guild.roles.cache.get(config.boost_bot_role_id);
    if (!boostBotRole) {
      await interaction.reply({ content: 'Boost bot role not found.', flags: MessageFlags.Ephemeral });
      return;
    }

    await interaction.deferReply({ flags: MessageFlags.Ephemeral });
    await interaction.guild.members.fetch();

    const boostBotMembers = boostBotRole.members;
    const expiredUsers = [];
    const activeUsers = [];

    for (const [memberId, member] of boostBotMembers) {
      if (member.premiumSince) {
        activeUsers.push(member);
      } else {
        expiredUsers.push(member);
      }
    }

    const fields = [
      { name: '✅ Still boosting', value: activeUsers.length > 0 ? activeUsers.map(m => m.user.tag).join('\n') : 'None', inline: true },
      { name: '❌ No longer boosting', value: expiredUsers.length > 0 ? expiredUsers.map(m => m.user.tag).join('\n') : 'None', inline: true },
    ];

    await interaction.editReply({
      embeds: [createEmbed({
        title: '🔍 Boost Bot Status',
        color: expiredUsers.length > 0 ? 0xef4444 : 0x22c55e,
        fields,
      })],
    });
  },
};
