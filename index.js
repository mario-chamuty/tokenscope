require('dotenv').config();

const { Client, GatewayIntentBits, Partials } = require('discord.js');
const { initialize } = require('./src/database');
const { loadEvents } = require('./src/handlers/eventHandler');
const { loadCommands } = require('./src/handlers/commandHandler');
const { startBoostChecker } = require('./src/features/boostManager');
const { startAntiNukeCleanup } = require('./src/features/protection');
const { startStatsUpdater } = require('./src/features/serverStats');

const client = new Client({
  intents: [
    GatewayIntentBits.Guilds,
    GatewayIntentBits.GuildMembers,
    GatewayIntentBits.GuildMessages,
    GatewayIntentBits.GuildVoiceStates,
    GatewayIntentBits.GuildModeration,
    GatewayIntentBits.MessageContent,
    GatewayIntentBits.DirectMessages,
    GatewayIntentBits.GuildPresences,
  ],
  partials: [
    Partials.Channel,
    Partials.Message,
    Partials.GuildMember,
  ],
});

// Initialize database
initialize();
console.log('[Database] Initialized');

// Load handlers
loadCommands(client);
loadEvents(client);

// Start scheduled tasks once ready
client.once('clientReady', () => {
  startBoostChecker(client);
  startAntiNukeCleanup();
  startStatsUpdater(client);
  console.log(`[CraftingBot] Logged in as ${client.user.tag}`);
});

// Prevent crashes from unhandled promise rejections
process.on('unhandledRejection', (error) => {
  console.error('[Unhandled Rejection]', error.message || error);
});

client.login(process.env.DISCORD_TOKEN);
