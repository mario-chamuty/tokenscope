const { SlashCommandBuilder, PermissionFlagsBits } = require('discord.js');
const { generateWelcomeImage, generateLeaveImage } = require('../features/welcomeImage');

module.exports = {
  data: new SlashCommandBuilder()
    .setName('testwelcome')
    .setDescription('Preview the welcome/leave images')
    .setDefaultMemberPermissions(PermissionFlagsBits.Administrator)
    .addStringOption(opt =>
      opt.setName('type')
        .setDescription('Which image to preview')
        .setRequired(true)
        .addChoices(
          { name: 'Welcome', value: 'welcome' },
          { name: 'Leave', value: 'leave' },
        )),

  async execute(interaction) {
    await interaction.deferReply();

    const type = interaction.options.getString('type');

    try {
      const image = type === 'welcome'
        ? await generateWelcomeImage(interaction.member)
        : await generateLeaveImage(interaction.member);

      await interaction.editReply({ files: [image] });
    } catch (err) {
      console.error('[TestWelcome]', err);
      await interaction.editReply({ content: `Error generating image: ${err.message}` });
    }
  },
};
