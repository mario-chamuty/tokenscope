const { handleVoiceStateUpdate } = require('../features/voiceManager');

module.exports = {
  name: 'voiceStateUpdate',
  async execute(oldState, newState, client) {
    try {
      await handleVoiceStateUpdate(oldState, newState, client);
    } catch (error) {
      console.error('[VoiceState Error]', error.message);
    }
  },
};
