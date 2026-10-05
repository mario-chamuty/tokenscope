const { generateLeaveImage } = require('../features/welcomeImage');
const { db } = require('../database');
const { t } = require('../i18n');
const { createEmbed, getGuildConfig } = require('../utils/helpers');

module.exports = {
  name: 'guildMemberRemove',
  async execute(member, client) {
    // Guard against partial members with missing user data
    if (!member.user) return;

    try {
      const config = getGuildConfig(db, member.guild.id);
      const lang = config.language || 'sk';

      // Send leave message with generated image
      if (config.leave_channel_id || config.welcome_channel_id) {
        const channelId = config.leave_channel_id || config.welcome_channel_id;
        const channel = member.guild.channels.cache.get(channelId);
        if (channel) {
          try {
            const leaveImage = await generateLeaveImage(member);

            const embed = createEmbed({
              title: t('leave.title', lang),
              description: t('leave.description', lang, {
                user: member.user.tag,
                count: member.guild.memberCount + 1,
                newCount: member.guild.memberCount,
              }),
              color: 0xe74c3c,
              timestamp: true,
            });
            embed.setImage('attachment://goodbye.png');

            await channel.send({ embeds: [embed], files: [leaveImage] }).catch(() => {});
          } catch (err) {
            console.error('[Leave] Image generation failed:', err.message);
            const embed = createEmbed({
              title: t('leave.title', lang),
              description: t('leave.description', lang, {
                user: member.user.tag,
                count: member.guild.memberCount + 1,
                newCount: member.guild.memberCount,
              }),
              color: 0xef4444,
              thumbnail: member.user.displayAvatarURL({ dynamic: true }),
            });
            await channel.send({ embeds: [embed] }).catch(() => {});
          }
        }
      }

      // Log member leave
      if (config.log_channel_id) {
        const logChannel = member.guild.channels.cache.get(config.log_channel_id);
        if (logChannel) {
          await logChannel.send({
            embeds: [createEmbed({
              description: t('logging.memberLeave', lang, { user: member.user.tag }),
              color: 0xef4444,
            })],
          }).catch(() => {});
        }
      }

      // Clean up onboarding state if any
      db.prepare('DELETE FROM onboarding_state WHERE guild_id = ? AND user_id = ?')
        .run(member.guild.id, member.id);
    } catch (error) {
      console.error('[GuildMemberRemove Error]', error.message);
    }
  },
};
