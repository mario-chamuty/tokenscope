const { MessageFlags } = require('discord.js');
const { handleOnboardingInteraction } = require('../features/onboarding');
const { handleSelfRoleInteraction } = require('../features/selfRoles');
const { handleBoostKickInteraction } = require('../features/boostManager');

module.exports = {
  name: 'interactionCreate',
  async execute(interaction, client) {
    // Slash commands
    if (interaction.isChatInputCommand()) {
      const command = client.commands.get(interaction.commandName);
      if (!command) return;

      try {
        await command.execute(interaction, client);
      } catch (error) {
        console.error(`[Command Error] ${interaction.commandName}:`, error);
        try {
          const reply = { content: '❌ An error occurred.', flags: MessageFlags.Ephemeral };
          if (interaction.replied || interaction.deferred) {
            await interaction.followUp(reply);
          } else {
            await interaction.reply(reply);
          }
        } catch {
          // Interaction expired
        }
      }
      return;
    }

    // Buttons and select menus
    if (interaction.isButton() || interaction.isStringSelectMenu()) {
      try {
        const customId = interaction.customId;

        if (customId.startsWith('onboard_')) {
          await handleOnboardingInteraction(interaction, client);
          return;
        }

        if (customId.startsWith('selfrole_') || customId.startsWith('suggestion_')) {
          await handleSelfRoleInteraction(interaction, client);
          return;
        }

        if (customId.startsWith('boost_kickall_')) {
          await handleBoostKickInteraction(interaction);
          return;
        }
      } catch (error) {
        console.error('[Interaction Error]', error);
        try {
          if (!interaction.replied && !interaction.deferred) {
            await interaction.reply({ content: '❌ An error occurred.', flags: MessageFlags.Ephemeral });
          }
        } catch {
          // Interaction expired
        }
      }
    }

    // Modal submissions
    if (interaction.isModalSubmit()) {
      try {
        if (interaction.customId === 'selfrole_suggest_modal') {
          await handleSelfRoleInteraction(interaction, client);
          return;
        }
      } catch (error) {
        console.error('[Modal Error]', error);
        try {
          if (!interaction.replied && !interaction.deferred) {
            await interaction.reply({ content: '❌ An error occurred.', flags: MessageFlags.Ephemeral });
          }
        } catch {
          // Interaction expired
        }
      }
    }
  },
};
