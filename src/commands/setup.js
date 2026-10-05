const { SlashCommandBuilder, PermissionFlagsBits, ChannelType, MessageFlags } = require('discord.js');
const { db } = require('../database');
const { t } = require('../i18n');
const { createEmbed, getGuildConfig, updateGuildConfig } = require('../utils/helpers');
const { sendSelfRoleEmbeds } = require('../features/selfRoles');
const { lockdownBoostBots } = require('../features/boostManager');
const { createStatsChannels, removeStatsChannels } = require('../features/serverStats');

module.exports = {
  data: new SlashCommandBuilder()
    .setName('setup')
    .setDescription('Configure CraftingBot for this server')
    .setDefaultMemberPermissions(PermissionFlagsBits.Administrator)
    .addSubcommand(sub =>
      sub.setName('welcome')
        .setDescription('Set the welcome channel')
        .addChannelOption(opt =>
          opt.setName('channel').setDescription('Welcome channel').setRequired(true)
            .addChannelTypes(ChannelType.GuildText)))
    .addSubcommand(sub =>
      sub.setName('leave')
        .setDescription('Set the leave channel')
        .addChannelOption(opt =>
          opt.setName('channel').setDescription('Leave channel').setRequired(true)
            .addChannelTypes(ChannelType.GuildText)))
    .addSubcommand(sub =>
      sub.setName('log')
        .setDescription('Set the audit log channel')
        .addChannelOption(opt =>
          opt.setName('channel').setDescription('Log channel').setRequired(true)
            .addChannelTypes(ChannelType.GuildText)))
    .addSubcommand(sub =>
      sub.setName('admin')
        .setDescription('Set the admin notification channel')
        .addChannelOption(opt =>
          opt.setName('channel').setDescription('Admin channel').setRequired(true)
            .addChannelTypes(ChannelType.GuildText)))
    .addSubcommand(sub =>
      sub.setName('selfroles')
        .setDescription('Set up the self-role channel and send role embeds')
        .addChannelOption(opt =>
          opt.setName('channel').setDescription('Self-role channel').setRequired(true)
            .addChannelTypes(ChannelType.GuildText)))
    .addSubcommand(sub =>
      sub.setName('voicehub')
        .setDescription('Set the voice hub channel (join to create personal channel)')
        .addChannelOption(opt =>
          opt.setName('channel').setDescription('Voice hub channel').setRequired(true)
            .addChannelTypes(ChannelType.GuildVoice)))
    .addSubcommand(sub =>
      sub.setName('boostrole')
        .setDescription('Set the boost bot role and lockdown its permissions')
        .addRoleOption(opt =>
          opt.setName('role').setDescription('Boost bot role').setRequired(true)))
    .addSubcommand(sub =>
      sub.setName('memberroles')
        .setDescription('Set full member and temporary member roles')
        .addRoleOption(opt =>
          opt.setName('full').setDescription('Full member role').setRequired(true))
        .addRoleOption(opt =>
          opt.setName('temp').setDescription('Temporary member role').setRequired(true)))
    .addSubcommand(sub =>
      sub.setName('language')
        .setDescription('Set bot language')
        .addStringOption(opt =>
          opt.setName('lang').setDescription('Language').setRequired(true)
            .addChoices(
              { name: 'Slovenčina', value: 'sk' },
              { name: 'English', value: 'en' },
            )))
    .addSubcommand(sub =>
      sub.setName('autorole')
        .setDescription('Set a role to auto-assign to every new member')
        .addRoleOption(opt =>
          opt.setName('role').setDescription('Role to auto-assign (leave empty to disable)')))
    .addSubcommand(sub =>
      sub.setName('stats')
        .setDescription('Create or remove server stats channels')
        .addStringOption(opt =>
          opt.setName('action').setDescription('Create or remove stats channels').setRequired(true)
            .addChoices(
              { name: 'Create', value: 'create' },
              { name: 'Remove', value: 'remove' },
            )))
    .addSubcommand(sub =>
      sub.setName('status')
        .setDescription('Show current bot configuration')),

  async execute(interaction) {
    const sub = interaction.options.getSubcommand();
    const guildId = interaction.guild.id;
    const config = getGuildConfig(db, guildId);
    const lang = config.language || 'sk';

    switch (sub) {
      case 'welcome': {
        const channel = interaction.options.getChannel('channel');
        updateGuildConfig(db, guildId, 'welcome_channel_id', channel.id);
        await interaction.reply({ content: t('admin.configUpdated', lang), flags: MessageFlags.Ephemeral });
        break;
      }
      case 'leave': {
        const channel = interaction.options.getChannel('channel');
        updateGuildConfig(db, guildId, 'leave_channel_id', channel.id);
        await interaction.reply({ content: t('admin.configUpdated', lang), flags: MessageFlags.Ephemeral });
        break;
      }
      case 'log': {
        const channel = interaction.options.getChannel('channel');
        updateGuildConfig(db, guildId, 'log_channel_id', channel.id);
        await interaction.reply({ content: t('admin.configUpdated', lang), flags: MessageFlags.Ephemeral });
        break;
      }
      case 'admin': {
        const channel = interaction.options.getChannel('channel');
        updateGuildConfig(db, guildId, 'admin_channel_id', channel.id);
        await interaction.reply({ content: t('admin.configUpdated', lang), flags: MessageFlags.Ephemeral });
        break;
      }
      case 'selfroles': {
        const channel = interaction.options.getChannel('channel');
        await interaction.deferReply({ flags: MessageFlags.Ephemeral });
        updateGuildConfig(db, guildId, 'selfrole_channel_id', channel.id);
        await sendSelfRoleEmbeds(channel, guildId);
        await interaction.editReply({ content: t('admin.configUpdated', lang) });
        break;
      }
      case 'voicehub': {
        const channel = interaction.options.getChannel('channel');
        updateGuildConfig(db, guildId, 'voice_hub_channel_id', channel.id);
        await interaction.reply({ content: t('admin.configUpdated', lang), flags: MessageFlags.Ephemeral });
        break;
      }
      case 'boostrole': {
        const role = interaction.options.getRole('role');
        await interaction.deferReply({ flags: MessageFlags.Ephemeral });
        updateGuildConfig(db, guildId, 'boost_bot_role_id', role.id);
        await lockdownBoostBots(interaction.guild);
        await interaction.editReply({ content: t('admin.configUpdated', lang) });
        break;
      }
      case 'memberroles': {
        const full = interaction.options.getRole('full');
        const temp = interaction.options.getRole('temp');
        updateGuildConfig(db, guildId, 'full_member_role_id', full.id);
        updateGuildConfig(db, guildId, 'temp_member_role_id', temp.id);
        await interaction.reply({ content: t('admin.configUpdated', lang), flags: MessageFlags.Ephemeral });
        break;
      }
      case 'language': {
        const newLang = interaction.options.getString('lang');
        updateGuildConfig(db, guildId, 'language', newLang);
        await interaction.reply({ content: t('admin.configUpdated', newLang), flags: MessageFlags.Ephemeral });
        break;
      }
      case 'autorole': {
        const role = interaction.options.getRole('role');
        if (role) {
          updateGuildConfig(db, guildId, 'auto_role_id', role.id);
          await interaction.reply({ content: `✅ Auto-role set to **${role.name}** – every new member will get this role.`, flags: MessageFlags.Ephemeral });
        } else {
          updateGuildConfig(db, guildId, 'auto_role_id', null);
          await interaction.reply({ content: '✅ Auto-role disabled.', flags: MessageFlags.Ephemeral });
        }
        break;
      }
      case 'stats': {
        const action = interaction.options.getString('action');
        await interaction.deferReply({ flags: MessageFlags.Ephemeral });

        if (action === 'create') {
          const existing = getGuildConfig(db, guildId);
          if (existing.stats_category_id) {
            await interaction.editReply({ content: 'Stats channels already exist. Remove them first with `/setup stats action:Remove`.' });
            break;
          }
          await createStatsChannels(interaction.guild);
          await interaction.editReply({ content: '✅ Server stats channels created! They update every 5 minutes.' });
        } else {
          await removeStatsChannels(interaction.guild);
          await interaction.editReply({ content: '✅ Server stats channels removed.' });
        }
        break;
      }
      case 'status': {
        const freshConfig = getGuildConfig(db, guildId);
        const fields = [
          { name: 'Language', value: freshConfig.language || 'sk', inline: true },
          { name: 'Welcome Channel', value: freshConfig.welcome_channel_id ? `<#${freshConfig.welcome_channel_id}>` : 'Not set', inline: true },
          { name: 'Leave Channel', value: freshConfig.leave_channel_id ? `<#${freshConfig.leave_channel_id}>` : 'Not set', inline: true },
          { name: 'Log Channel', value: freshConfig.log_channel_id ? `<#${freshConfig.log_channel_id}>` : 'Not set', inline: true },
          { name: 'Admin Channel', value: freshConfig.admin_channel_id ? `<#${freshConfig.admin_channel_id}>` : 'Not set', inline: true },
          { name: 'Self-role Channel', value: freshConfig.selfrole_channel_id ? `<#${freshConfig.selfrole_channel_id}>` : 'Not set', inline: true },
          { name: 'Voice Hub', value: freshConfig.voice_hub_channel_id ? `<#${freshConfig.voice_hub_channel_id}>` : 'Not set', inline: true },
          { name: 'Boost Bot Role', value: freshConfig.boost_bot_role_id ? `<@&${freshConfig.boost_bot_role_id}>` : 'Not set', inline: true },
          { name: 'Full Member Role', value: freshConfig.full_member_role_id ? `<@&${freshConfig.full_member_role_id}>` : 'Not set', inline: true },
          { name: 'Temp Member Role', value: freshConfig.temp_member_role_id ? `<@&${freshConfig.temp_member_role_id}>` : 'Not set', inline: true },
          { name: 'Auto Role', value: freshConfig.auto_role_id ? `<@&${freshConfig.auto_role_id}>` : 'Not set', inline: true },
          { name: 'Stats Channels', value: freshConfig.stats_category_id ? '✅ Active' : 'Not set', inline: true },
        ];

        await interaction.reply({
          embeds: [createEmbed({
            title: '⚙️ CraftingBot Configuration',
            color: 0x7c3aed,
            fields,
          })],
          flags: MessageFlags.Ephemeral,
        });
        break;
      }
    }
  },
};
