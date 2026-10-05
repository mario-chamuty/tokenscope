const { startOnboarding } = require('../features/onboarding');
const { generateWelcomeImage } = require('../features/welcomeImage');
const { db } = require('../database');
const { t } = require('../i18n');
const { createEmbed, getGuildConfig } = require('../utils/helpers');

module.exports = {
  name: 'guildMemberAdd',
  async execute(member, client) {
    try {
      const config = getGuildConfig(db, member.guild.id);
      const lang = config.language || 'sk';

      // Send welcome message with generated image
      if (config.welcome_channel_id) {
        const channel = member.guild.channels.cache.get(config.welcome_channel_id);
        if (channel) {
          try {
            const welcomeImage = await generateWelcomeImage(member);

            const embed = createEmbed({
              title: t('welcome.title', lang),
              description: t('welcome.description', lang, {
                user: member.toString(),
                server: member.guild.name,
                count: member.guild.memberCount,
              }),
              color: 0x6b8e23,
              footer: t('welcome.footer', lang),
              timestamp: true,
            });
            embed.setImage('attachment://welcome.png');

            await channel.send({ embeds: [embed], files: [welcomeImage] }).catch(() => {});
          } catch (err) {
            console.error('[Welcome] Image generation failed:', err.message);
            const embed = createEmbed({
              title: t('welcome.title', lang),
              description: t('welcome.description', lang, {
                user: member.toString(),
                server: member.guild.name,
                count: member.guild.memberCount,
              }),
              color: 0x6b8e23,
              thumbnail: member.user.displayAvatarURL({ dynamic: true }),
              footer: t('welcome.footer', lang),
            });
            await channel.send({ embeds: [embed] }).catch(() => {});
          }
        }
      }

      // Auto-role on join
      if (config.auto_role_id) {
        const autoRole = member.guild.roles.cache.get(config.auto_role_id);
        if (autoRole) {
          await member.roles.add(autoRole, 'Auto-role on join').catch(err =>
            console.error('[AutoRole] Failed:', err.message));
        }
      }

      // Log member join
      if (config.log_channel_id) {
        const logChannel = member.guild.channels.cache.get(config.log_channel_id);
        if (logChannel) {
          await logChannel.send({
            embeds: [createEmbed({
              description: t('logging.memberJoin', lang, { user: member.user.tag }),
              color: 0x22c55e,
            })],
          }).catch(() => {});
        }
      }

      // Start DM onboarding
      await startOnboarding(member, client);
    } catch (error) {
      console.error('[GuildMemberAdd Error]', error.message);
    }
  },
};
