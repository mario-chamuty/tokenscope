const { SlashCommandBuilder, PermissionFlagsBits, MessageFlags } = require('discord.js');
const { db } = require('../database');
const { t } = require('../i18n');
const { getGuildConfig } = require('../utils/helpers');
const { sendSelfRoleEmbeds } = require('../features/selfRoles');

module.exports = {
  data: new SlashCommandBuilder()
    .setName('roles')
    .setDescription('Manage self-assignable roles')
    .setDefaultMemberPermissions(PermissionFlagsBits.ManageRoles)
    .addSubcommand(sub =>
      sub.setName('add')
        .setDescription('Add a role to the self-role system')
        .addRoleOption(opt =>
          opt.setName('role').setDescription('The role to add').setRequired(true))
        .addStringOption(opt =>
          opt.setName('category').setDescription('Role category').setRequired(true)
            .addChoices(
              { name: 'Game', value: 'game' },
              { name: 'Interest', value: 'interest' },
              { name: 'Notification', value: 'notification' },
            ))
        .addStringOption(opt =>
          opt.setName('label').setDescription('Button label (defaults to role name)'))
        .addStringOption(opt =>
          opt.setName('emoji').setDescription('Button emoji (e.g. 🎮)')))
    .addSubcommand(sub =>
      sub.setName('remove')
        .setDescription('Remove a role from the self-role system')
        .addRoleOption(opt =>
          opt.setName('role').setDescription('The role to remove').setRequired(true)))
    .addSubcommand(sub =>
      sub.setName('list')
        .setDescription('List all self-assignable roles'))
    .addSubcommand(sub =>
      sub.setName('refresh')
        .setDescription('Refresh the self-role embeds in the configured channel')),

  async execute(interaction) {
    const sub = interaction.options.getSubcommand();
    const guildId = interaction.guild.id;
    const config = getGuildConfig(db, guildId);
    const lang = config.language || 'sk';

    switch (sub) {
      case 'add': {
        const role = interaction.options.getRole('role');
        const category = interaction.options.getString('category');
        const label = interaction.options.getString('label') || role.name;
        const emoji = interaction.options.getString('emoji') || null;

        db.prepare(
          'INSERT OR REPLACE INTO self_roles (guild_id, role_id, category, label, emoji) VALUES (?, ?, ?, ?, ?)'
        ).run(guildId, role.id, category, label, emoji);

        await interaction.reply({
          content: t('admin.roleCreated', lang, { role: role.name }),
          flags: MessageFlags.Ephemeral,
        });
        break;
      }
      case 'remove': {
        const role = interaction.options.getRole('role');
        db.prepare('DELETE FROM self_roles WHERE guild_id = ? AND role_id = ?')
          .run(guildId, role.id);
        await interaction.reply({
          content: t('admin.roleDeleted', lang, { role: role.name }),
          flags: MessageFlags.Ephemeral,
        });
        break;
      }
      case 'list': {
        const roles = db.prepare('SELECT * FROM self_roles WHERE guild_id = ? ORDER BY category, label')
          .all(guildId);

        if (roles.length === 0) {
          await interaction.reply({ content: 'No self-assignable roles configured.', flags: MessageFlags.Ephemeral });
          return;
        }

        const grouped = {};
        for (const r of roles) {
          if (!grouped[r.category]) grouped[r.category] = [];
          grouped[r.category].push(`${r.emoji || ''} <@&${r.role_id}> (${r.label})`);
        }

        let desc = '';
        for (const [cat, list] of Object.entries(grouped)) {
          desc += `**${cat.toUpperCase()}**\n${list.join('\n')}\n\n`;
        }

        await interaction.reply({ content: desc, flags: MessageFlags.Ephemeral });
        break;
      }
      case 'refresh': {
        if (!config.selfrole_channel_id) {
          await interaction.reply({ content: 'Self-role channel not configured. Use `/setup selfroles` first.', flags: MessageFlags.Ephemeral });
          return;
        }

        const channel = interaction.guild.channels.cache.get(config.selfrole_channel_id);
        if (!channel) {
          await interaction.reply({ content: 'Self-role channel not found.', flags: MessageFlags.Ephemeral });
          return;
        }

        await interaction.deferReply({ flags: MessageFlags.Ephemeral });

        // Clear old bot messages
        const messages = await channel.messages.fetch({ limit: 50 });
        const botMessages = messages.filter(m => m.author.id === interaction.client.user.id);
        await Promise.all(botMessages.map(m => m.delete().catch(() => {})));

        await sendSelfRoleEmbeds(channel, guildId);
        await interaction.editReply({ content: t('admin.configUpdated', lang) });
        break;
      }
    }
  },
};
