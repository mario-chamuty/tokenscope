const { checkSpam, checkWordFilter } = require('../features/protection');

module.exports = {
  name: 'messageCreate',
  async execute(message, client) {
    if (message.author.bot) return;
    if (!message.guild) return;

    // Check word filter first
    const filtered = await checkWordFilter(message);
    if (filtered) return;

    // Check spam
    await checkSpam(message);
  },
};
