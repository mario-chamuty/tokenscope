module.exports = {
  // Onboarding
  onboarding: {
    welcome: 'Hey {user}! Welcome to **{server}**! 🎮',
    welcomeDesc: 'Glad to have you here! Answer a few questions so I can set up your roles.',
    askGames: 'What games do you play? Select all that apply:',
    askInterests: 'What are your interests? Select all that apply:',
    askMemberType: 'Do you want to be a full member or just visiting temporarily?',
    memberTypeFull: 'Full member',
    memberTypeTemp: 'Temporary visitor',
    onboardingComplete: 'Done! Your roles have been set up. Enjoy the server! 🎉',
    onboardingTimeout: 'Onboarding timed out. You can set your roles manually in the self-role channel.',
    dmDisabled: '{user}, I couldn\'t send you a DM. Please enable DMs from this server and try again, or set your roles in the self-role channel.',
  },

  // Self-roles
  selfroles: {
    title: '🎮 Choose your roles',
    description: 'Click the buttons below to add or remove roles.',
    gameRolesTitle: '🎮 Game Roles',
    interestRolesTitle: '💡 Interests',
    notificationRolesTitle: '🔔 Notifications',
    roleAdded: 'Role **{role}** has been added!',
    roleRemoved: 'Role **{role}** has been removed!',
    suggestionSent: 'Your game suggestion **{game}** has been sent to admins for approval.',
    suggestionApproved: 'Game suggestion **{game}** has been approved!',
    suggestionDenied: 'Game suggestion **{game}** has been denied.',
  },

  // Voice channels
  voice: {
    personalChannel: 'Channel – {user}',
    clonedChannel: '{channel} #{number}',
    hubChannelName: '➕ Create Channel',
  },

  // Boost management
  boost: {
    expiredTitle: '⚠️ Boost Expired',
    expiredDesc: 'The following users with serverboostbot role are no longer boosting:\n{users}\n\nPlease kick them.',
    noExpired: 'All boost bots are still boosting.',
  },

  // Protection
  protection: {
    spamDetected: '🚫 {user} has been muted for spam.',
    nukeDetected: '🚨 **NUKE DETECTED** – User {user} performed mass deletions! Permissions have been frozen.',
    nukeAlert: '🚨 Anti-nuke system has been activated! Check the audit log.',
    wordFiltered: '🚫 Message from {user} was deleted (forbidden word).',
  },

  // Logging
  logging: {
    memberJoin: '📥 **{user}** joined the server',
    memberLeave: '📤 **{user}** left the server',
    roleAdd: '➕ Role **{role}** added to **{user}**',
    roleRemove: '➖ Role **{role}** removed from **{user}**',
    messageDelete: '🗑️ Message from **{user}** was deleted in {channel}',
    channelCreate: '📁 Channel **{channel}** was created',
    channelDelete: '📁 Channel **{channel}** was deleted',
    roleCreate: '🏷️ Role **{role}** was created',
    roleDelete: '🏷️ Role **{role}** was deleted',
    ban: '🔨 **{user}** was banned',
    kick: '👢 **{user}** was kicked',
  },

  // Welcome/Leave
  welcome: {
    title: 'Welcome to the server! 🎉',
    description: 'Hey {user}, welcome to **{server}**! You are our **{count}th** member!',
    footer: 'Check your DMs for role setup.',
  },
  leave: {
    title: 'Member Left',
    description: '**{user}** has left the server. We were {count}, now we are {newCount}.',
  },

  // Admin commands
  admin: {
    configUpdated: '✅ Configuration has been updated.',
    roleCreated: '✅ Role **{role}** has been created and added to the self-role system.',
    roleDeleted: '✅ Role **{role}** has been removed from the self-role system.',
    noPermission: '❌ You don\'t have permission for this command.',
    wordAdded: '✅ Word **{word}** has been added to the filter.',
    wordRemoved: '✅ Word **{word}** has been removed from the filter.',
  },

  // General
  general: {
    error: 'An error occurred. Please try again later.',
    botName: 'CraftingBot',
  },
};
