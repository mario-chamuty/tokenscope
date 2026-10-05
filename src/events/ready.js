const { ActivityType } = require('discord.js');

module.exports = {
  name: 'clientReady',
  once: true,
  execute(client) {
    client.user.setActivity('over the server', { type: ActivityType.Watching });
    console.log(`[Ready] Serving ${client.guilds.cache.size} guild(s)`);
  },
};
