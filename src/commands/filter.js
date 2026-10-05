const { SlashCommandBuilder, PermissionFlagsBits, MessageFlags } = require('discord.js');
const { db } = require('../database');
const { t } = require('../i18n');
const { getGuildConfig } = require('../utils/helpers');

module.exports = {
  data: new SlashCommandBuilder()
    .setName('filter')
    .setDescription('Manage the word filter')
    .setDefaultMemberPermissions(PermissionFlagsBits.ManageMessages)
    .addSubcommand(sub =>
      sub.setName('add')
        .setDescription('Add a word to the filter')
        .addStringOption(opt =>
          opt.setName('word').setDescription('Word to filter').setRequired(true)))
    .addSubcommand(sub =>
      sub.setName('remove')
        .setDescription('Remove a word from the filter')
        .addStringOption(opt =>
          opt.setName('word').setDescription('Word to unfilter').setRequired(true)))
    .addSubcommand(sub =>
      sub.setName('list')
        .setDescription('List all filtered words')),

  async execute(interaction) {
    const sub = interaction.options.getSubcommand();
    const guildId = interaction.guild.id;
    const config = getGuildConfig(db, guildId);
    const lang = config.language || 'sk';

    switch (sub) {
      case 'add': {
        const word = interaction.options.getString('word').toLowerCase();
        db.prepare('INSERT OR IGNORE INTO filtered_words (guild_id, word) VALUES (?, ?)')
          .run(guildId, word);
        await interaction.reply({
          content: t('admin.wordAdded', lang, { word }),
          flags: MessageFlags.Ephemeral,
        });
        break;
      }
      case 'remove': {
        const word = interaction.options.getString('word').toLowerCase();
        db.prepare('DELETE FROM filtered_words WHERE guild_id = ? AND word = ?')
          .run(guildId, word);
        await interaction.reply({
          content: t('admin.wordRemoved', lang, { word }),
          flags: MessageFlags.Ephemeral,
        });
        break;
      }
      case 'list': {
        const words = db.prepare('SELECT word FROM filtered_words WHERE guild_id = ?')
          .all(guildId);
        if (words.length === 0) {
          await interaction.reply({ content: 'No filtered words.', flags: MessageFlags.Ephemeral });
          return;
        }
        const list = words.map(w => `\`${w.word}\``).join(', ');
        await interaction.reply({ content: `Filtered words: ${list}`, flags: MessageFlags.Ephemeral });
        break;
      }
    }
  },
};
