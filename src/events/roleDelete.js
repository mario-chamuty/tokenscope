const { db } = require('../database');
const { trackNukeAction } = require('../features/protection');
const { t } = require('../i18n');
const { createEmbed, getGuildConfig } = require('../utils/helpers');

module.exports = {
  name: 'roleDelete',
  async execute(role, client) {
    if (!role.guild) return;

    try {
      const config = getGuildConfig(db, role.guild.id);
      const lang = config.language || 'sk';

      // Clean up self_roles records
      db.prepare('DELETE FROM self_roles WHERE role_id = ? AND guild_id = ?').run(role.id, role.guild.id);

      // Log
      if (config.log_channel_id) {
        const logChannel = role.guild.channels.cache.get(config.log_channel_id);
        if (logChannel) {
          await logChannel.send({
            embeds: [createEmbed({
              description: t('logging.roleDelete', lang, { role: role.name }),
              color: 0xef4444,
            })],
          }).catch(() => {});
        }
      }

      // Anti-nuke tracking
      const auditLogs = await role.guild.fetchAuditLogs({
        type: 32, // RoleDelete
        limit: 1,
      }).catch(() => null);

      if (auditLogs) {
        const entry = auditLogs.entries.first();
        if (entry && entry.executor && !entry.executor.bot) {
          await trackNukeAction(role.guild, entry.executor.id, 'role_delete');
        }
      }
    } catch (error) {
      console.error('[RoleDelete Error]', error.message);
    }
  },
};
