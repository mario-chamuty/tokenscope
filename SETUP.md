# CraftingBot – Setup Guide

## 1. Discord Developer Portal Setup

1. Go to https://discord.com/developers/applications
2. Select your application (ID: `1491822513154424842`)
3. Go to **Bot** section:
   - Enable **Privileged Gateway Intents**:
     - ✅ PRESENCE INTENT
     - ✅ SERVER MEMBERS INTENT
     - ✅ MESSAGE CONTENT INTENT
   - Save changes

## 2. Bot Permissions

The bot needs these permissions (permission integer: `1644972474487`):

- Manage Roles
- Manage Channels
- Kick Members
- Ban Members
- View Channels
- Send Messages
- Manage Messages
- Embed Links
- Read Message History
- Mention Everyone
- Use External Emojis
- Add Reactions
- Connect
- Move Members
- Mute Members
- Moderate Members (for timeouts)
- View Audit Log

## 3. OAuth2 Invite Link

Since this is a private bot, generate the invite link manually:

1. Go to **OAuth2 > URL Generator** in the Developer Portal
2. Select scopes: `bot`, `applications.commands`
3. Select the permissions listed above
4. Or use this pre-built URL (replace permissions if needed):

```
https://discord.com/api/oauth2/authorize?client_id=1491822513154424842&permissions=1644972474487&scope=bot%20applications.commands
```

5. Open this URL in your browser, select your server, and authorize

## 4. Running the Bot

```bash
# Install dependencies
npm install

# Deploy slash commands (do this once, or after adding new commands)
npm run deploy-commands

# Start the bot
npm start

# Development mode (auto-restart on file changes)
npm run dev
```

## 5. Server Configuration

Once the bot is running and in your server, use these slash commands to configure it:

### Required setup (run these first):

```
/setup admin #admin-channel         – Where boost alerts and nuke alerts go
/setup welcome #welcome             – Where welcome messages are posted
/setup leave #leave                 – Where leave messages go (or same as welcome)
/setup log #bot-log                 – Audit log channel for all mod actions
/setup selfroles #self-roles        – Self-role channel (bot will post role embeds)
/setup voicehub [voice channel]     – Voice hub channel (join to create personal channel)
/setup boostrole @serverboostbot    – The G2A boost bot role
/setup memberroles @Full @Temp              – Member type roles
/setup language sk                  – Set language (sk/en)
```

### Check current config:

```
/setup status
```

### Add game roles for self-role system:

```
/roles add @Minecraft game "Minecraft" 🎮
/roles add @Valorant game "Valorant" 🔫
/roles add @LeagueOfLegends game "LoL" ⚔️
```

### Add other role types:

```
/roles add @Red color "Red" 🔴
/roles add @PC platform "PC" 🖥️
/roles add @PlayStation platform "PlayStation" 🎮
/roles add @Announcements notification "Announcements" 📢
```

### Refresh self-role embeds after changes:

```
/roles refresh
```

### Mark voice channels for auto-cloning:

```
/voicemonitor add #gaming-voice
/voicemonitor add #chill-voice
```

### Word filter:

```
/filter add badword
/filter list
/filter remove badword
```

### Manual boost check:

```
/boostcheck
```

## 6. How Features Work

### Onboarding
When a new member joins, the bot DMs them with:
1. Game selection (from configured game roles)
2. Full member vs temporary visitor choice
3. Gender role (optional)

### Self-Roles
The self-role channel has button embeds organized by category. Users click to toggle roles. There is a "Suggest a game" button that sends suggestions to the admin channel for approval.

### Temp Voice Channels
- **Hub**: A voice channel that, when joined, creates a personal temp channel and moves the user there. The user gets Manage/Move/Mute permissions in their channel. Channel deletes when empty.
- **Auto-clone**: Monitored voice channels automatically clone when full. Clones delete when empty.

### Boost Bot Management
Every hour, the bot checks all members with the serverboostbot role. If they are no longer boosting (premiumSince is null), it alerts the admin channel to kick them. The serverboostbot role is denied ViewChannel on all channels.

### Anti-Spam
- Rate limit: 5 messages per 5 seconds = auto-mute (10 min timeout)
- Duplicate detection: 3 identical messages in a row = auto-mute
- Mention spam: 5+ mentions in one message = auto-mute

### Anti-Nuke
Tracks channel and role deletions per user. If 5+ destructive actions happen within 10 seconds, the bot:
1. Removes all admin/manage roles from the offender
2. Alerts the admin channel with @here

### Word Filter
Admin-configurable list of forbidden words. Messages containing them are auto-deleted and logged.

## 7. File Structure

```
CraftingBot/
├── src/
│   ├── index.js              – Entry point
│   ├── deploy-commands.js    – Slash command registration
│   ├── commands/
│   │   ├── setup.js          – /setup command (all config)
│   │   ├── roles.js          – /roles command (self-role mgmt)
│   │   ├── filter.js         – /filter command (word filter)
│   │   ├── voicemonitor.js   – /voicemonitor command
│   │   └── boostcheck.js     – /boostcheck command
│   ├── events/
│   │   ├── ready.js
│   │   ├── guildMemberAdd.js
│   │   ├── guildMemberRemove.js
│   │   ├── messageCreate.js
│   │   ├── interactionCreate.js
│   │   ├── voiceStateUpdate.js
│   │   ├── channelDelete.js
│   │   └── roleDelete.js
│   ├── features/
│   │   ├── onboarding.js
│   │   ├── selfRoles.js
│   │   ├── voiceManager.js
│   │   ├── boostManager.js
│   │   └── protection.js
│   ├── handlers/
│   │   ├── eventHandler.js
│   │   └── commandHandler.js
│   ├── database/
│   │   └── index.js
│   ├── i18n/
│   │   ├── index.js
│   │   ├── sk.js
│   │   └── en.js
│   └── utils/
│       └── helpers.js
├── data/                     – SQLite database (auto-created)
├── .env                      – Bot token and config
├── .env.example
├── package.json
└── SETUP.md                  – This file
```
