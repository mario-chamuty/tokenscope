const { SlashCommandBuilder, PermissionFlagsBits, ChannelType, MessageFlags } = require('discord.js');
const { db } = require('../database');
const { getGuildConfig } = require('../utils/helpers');

module.exports = {
  data: new SlashCommandBuilder()
    .setName('voicemonitor')
    .setDescription('Manage auto-cloning voice channels')
    .setDefaultMemberPermissions(PermissionFlagsBits.ManageChannels)
    .addSubcommand(sub =>
      sub.setName('add')
        .setDescription('Mark a voice channel for auto-cloning when full')
        .addChannelOption(opt =>
          opt.setName('channel').setDescription('Voice channel to monitor').setRequired(true)
            .addChannelTypes(ChannelType.GuildVoice)))
    .addSubcommand(sub =>
      sub.setName('remove')
        .setDescription('Stop monitoring a voice channel')
        .addChannelOption(opt =>
          opt.setName('channel').setDescription('Voice channel to stop monitoring').setRequired(true)
            .addChannelTypes(ChannelType.GuildVoice)))
    .addSubcommand(sub =>
      sub.setName('list')
        .setDescription('List all monitored voice channels')),

  async execute(interaction) {
    const sub = interaction.options.getSubcommand();
    const guildId = interaction.guild.id;

    switch (sub) {
      case 'add': {
        const channel = interaction.options.getChannel('channel');
        db.prepare('INSERT OR IGNORE INTO monitored_voice_channels (channel_id, guild_id) VALUES (?, ?)')
          .run(channel.id, guildId);
        await interaction.reply({ content: `✅ Now monitoring **${channel.name}** for auto-cloning.`, flags: MessageFlags.Ephemeral });
        break;
      }
      case 'remove': {
        const channel = interaction.options.getChannel('channel');
        db.prepare('DELETE FROM monitored_voice_channels WHERE channel_id = ? AND guild_id = ?')
          .run(channel.id, guildId);
        await interaction.reply({ content: `✅ Stopped monitoring **${channel.name}**.`, flags: MessageFlags.Ephemeral });
        break;
      }
      case 'list': {
        const channels = db.prepare('SELECT channel_id FROM monitored_voice_channels WHERE guild_id = ?')
          .all(guildId);
        if (channels.length === 0) {
          await interaction.reply({ content: 'No monitored voice channels.', flags: MessageFlags.Ephemeral });
          return;
        }
        const list = channels.map(c => `<#${c.channel_id}>`).join('\n');
        await interaction.reply({ content: `Monitored channels:\n${list}`, flags: MessageFlags.Ephemeral });
        break;
      }
    }
  },
};
