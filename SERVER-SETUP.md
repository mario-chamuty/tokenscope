# crafting.sk – Discord Server Setup Guide

## Step 1: Create Roles

Create these roles in **Server Settings > Roles** (order matters – drag the bot role above all these):

| Role name        | Color   | Purpose                        |
|------------------|---------|--------------------------------|
| Člen             | green   | Full/permanent member          |
| Návštevník       | gray    | Temporary visitor              |
| serverboostbot   | pink    | G2A paid boost bot accounts    |
| Nováčik          | white   | Auto-assigned to every new member |

> **Important:** Make sure the **CraftingBot** role is **above** all these roles in the role list, otherwise it can't assign them.

## Step 2: Create Channels

Create these channels:

**Text channels:**
| Channel name     | Category     | Purpose                          |
|------------------|--------------|----------------------------------|
| #vitajte         | Info         | Welcome & leave messages         |
| #self-role       | Info         | Self-role buttons (bot manages)  |
| #bot-log         | Admin        | Audit log (joins, leaves, mods)  |
| #admin           | Admin        | Admin alerts (boost, nuke, suggestions) |

> Make #admin and #bot-log visible only to admins.

**Voice channels:**
| Channel name          | Category | Purpose                              |
|-----------------------|----------|--------------------------------------|
| ➕ Vytvor kanál       | Voice    | Hub – join to create personal channel |

> Set a user limit on any voice channels you want auto-cloned (e.g. Gaming 🎮 with limit 5).

## Step 3: Deploy Commands & Start Bot

```bash
npm run deploy-commands
npm start
```

## Step 4: Run Setup Commands (in order)

Copy-paste these into Discord one by one:

### 4.1 – Language
```
/setup language lang:sk
```

### 4.2 – Channels
```
/setup welcome channel:#vitajte
/setup leave channel:#vitajte
/setup log channel:#bot-log
/setup admin channel:#admin
```

### 4.3 – Member & Auto Roles
```
/setup memberroles full:@Člen temp:@Návštevník
/setup autorole role:@Nováčik
```

### 4.4 – Boost Bot Role
```
/setup boostrole role:@serverboostbot
```
> This will lock the serverboostbot role out of all channels automatically.

### 4.5 – Self-Role Channel
```
/setup selfroles channel:#self-role
```
> The bot will post role embeds here. They'll be empty until you add roles (step 5).

### 4.6 – Voice Hub
```
/setup voicehub channel:➕ Vytvor kanál
```

### 4.7 – Server Stats
```
/setup stats action:Create
```
> Creates a "📊 Server Stats" category with member count, online count, and boost count channels. Updates every 5 minutes.

### 4.8 – Monitor Voice Channels for Auto-Clone (optional)
For each voice channel you want auto-cloned when full:
```
/voicemonitor add channel:#gaming-voice
```

## Step 5: Add Roles (when ready)

### Game roles
```
/roles add role:@Minecraft category:Game label:Minecraft emoji:⛏️
/roles add role:@Valorant category:Game label:Valorant emoji:🔫
```

### Interest roles
```
/roles add role:@Programovanie category:Interest label:Programovanie emoji:💻
/roles add role:@Hudba category:Interest label:Hudba emoji:🎵
```

### Notification roles
```
/roles add role:@Oznamy category:Notification label:Oznamy emoji:📢
/roles add role:@Eventy category:Notification label:Eventy emoji:🎉
/roles add role:@Súťaže category:Notification label:Súťaže emoji:🏆
```

> After adding roles, refresh the self-role channel:
```
/roles refresh
```

## Step 6: Word Filter (optional)
```
/filter add slovo1
/filter add slovo2
```

## Step 7: Verify Setup
```
/setup status
```
> Check that all channels and roles show as configured.

### Test welcome image:
```
/testwelcome type:Welcome
/testwelcome type:Leave
```

## Quick Reference – All Commands

| Command | What it does |
|---------|-------------|
| `/setup welcome/leave/log/admin` | Set channel |
| `/setup selfroles` | Set self-role channel + post embeds |
| `/setup voicehub` | Set voice creation hub |
| `/setup boostrole` | Set + lock down boost bot role |
| `/setup memberroles` | Set member type roles |
| `/setup autorole` | Set auto-assign role for new members |
| `/setup stats` | Create/remove stats channels |
| `/setup language` | Switch SK/EN |
| `/setup status` | Show current config |
| `/roles add/remove/list/refresh` | Manage self-assignable roles |
| `/voicemonitor add/remove/list` | Manage auto-clone channels |
| `/filter add/remove/list` | Manage word filter |
| `/boostcheck` | Manual boost bot status check |
| `/testwelcome` | Preview welcome/leave images |
