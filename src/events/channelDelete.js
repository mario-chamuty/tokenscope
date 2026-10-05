const { db } = require('../database');
const { trackNukeAction } = require('../features/protection');
const { t } = require('../i18n');
const { createEmbed, getGuildConfig } = require('../utils/helpers');

module.exports = {
  name: 'channelDelete',
  async execute(channel, client) {
    if (!channel.guild) return;

    try {
      const config = getGuildConfig(db, channel.guild.id);
      const lang = config.language || 'sk';

      // Clean up temp voice channel records
      db.prepare('DELETE FROM temp_voice_channels WHERE channel_id = ?').run(channel.id);
      db.prepare('DELETE FROM monitored_voice_channels WHERE channel_id = ?').run(channel.id);

      // Log channel deletion
      if (config.log_channel_id) {
        const logChannel = channel.guild.channels.cache.get(config.log_channel_id);
        if (logChannel) {
          await logChannel.send({
            embeds: [createEmbed({
              description: t('logging.channelDelete', lang, { channel: channel.name }),
              color: 0xef4444,
            })],
          }).catch(() => {});
        }
      }

      // Anti-nuke: track who deleted it
      const auditLogs = await channel.guild.fetchAuditLogs({
        type: 12, // ChannelDelete
        limit: 1,
      }).catch(() => null);

      if (auditLogs) {
        const entry = auditLogs.entries.first();
        if (entry && entry.executor && !entry.executor.bot) {
          await trackNukeAction(channel.guild, entry.executor.id, 'channel_delete');
        }
      }
    } catch (error) {
      console.error('[ChannelDelete Error]', error.message);
    }
  },
};
