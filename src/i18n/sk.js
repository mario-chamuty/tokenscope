module.exports = {
  // Onboarding
  onboarding: {
    welcome: 'Ahoj {user}! Vitaj na **{server}**! 🎮',
    welcomeDesc: 'Rád ťa tu vidím! Odpovej na pár otázok, aby som ti mohol nastaviť server.',
    askGames: 'Aké hry hráš? Vyber všetky, ktoré platia:',
    askInterests: 'Čo ťa zaujíma? Vyber všetky, ktoré platia:',
    askMemberType: 'Chceš byť stály člen alebo si tu len na návštevu?',
    memberTypeFull: 'Stály člen',
    memberTypeTemp: 'Dočasný návštevník',
    onboardingComplete: 'Hotovo! Tvoje roly boli nastavené. Užívaj si server! 🎉',
    onboardingTimeout: 'Onboarding vypršal. Môžeš si roly nastaviť manuálne v kanáli self-role.',
    dmDisabled: '{user}, nemôžem ti poslať správu. Prosím, povol si DM z tohto servera a skús znova, alebo si nastav roly v kanáli self-role.',
  },

  // Self-roles
  selfroles: {
    title: '🎮 Vyber si svoje roly',
    description: 'Klikni na tlačidlá nižšie pre priradenie alebo odobranie rolí.',
    gameRolesTitle: '🎮 Herné roly',
    interestRolesTitle: '💡 Záujmy',
    notificationRolesTitle: '🔔 Notifikácie',
    roleAdded: 'Rola **{role}** bola pridaná!',
    roleRemoved: 'Rola **{role}** bola odobraná!',
    suggestionSent: 'Tvoj návrh hry **{game}** bol odoslaný adminom na schválenie.',
    suggestionApproved: 'Návrh hry **{game}** bol schválený!',
    suggestionDenied: 'Návrh hry **{game}** bol zamietnutý.',
  },

  // Voice channels
  voice: {
    personalChannel: 'Kanál – {user}',
    clonedChannel: '{channel} #{number}',
    hubChannelName: '➕ Vytvor kanál',
  },

  // Boost management
  boost: {
    expiredTitle: '⚠️ Boost vypršal',
    expiredDesc: 'Nasledujúci používatelia s rolou serverboostbot už neboostujú server:\n{users}\n\nProsím, vykopnite ich.',
    noExpired: 'Všetci boost boti stále boostujú.',
  },

  // Protection
  protection: {
    spamDetected: '🚫 {user} bol stíšený za spam.',
    nukeDetected: '🚨 **NUKE DETEKCIA** – Používateľ {user} vykonal masové mazanie! Oprávnenia boli zmrazené.',
    nukeAlert: '🚨 Anti-nuke systém bol aktivovaný! Skontrolujte audit log.',
    wordFiltered: '🚫 Správa od {user} bola odstránená (zakázané slovo).',
  },

  // Logging
  logging: {
    memberJoin: '📥 **{user}** sa pripojil na server',
    memberLeave: '📤 **{user}** opustil server',
    roleAdd: '➕ Rola **{role}** pridaná používateľovi **{user}**',
    roleRemove: '➖ Rola **{role}** odobraná používateľovi **{user}**',
    messageDelete: '🗑️ Správa od **{user}** bola zmazaná v {channel}',
    channelCreate: '📁 Kanál **{channel}** bol vytvorený',
    channelDelete: '📁 Kanál **{channel}** bol zmazaný',
    roleCreate: '🏷️ Rola **{role}** bola vytvorená',
    roleDelete: '🏷️ Rola **{role}** bola zmazaná',
    ban: '🔨 **{user}** bol zabanovaný',
    kick: '👢 **{user}** bol vykopnutý',
  },

  // Welcome/Leave
  welcome: {
    title: 'Vitaj na serveri! 🎉',
    description: 'Ahoj {user}, vitaj na **{server}**! Si náš **{count}.** člen!',
    footer: 'Skontroluj si správy pre nastavenie rolí.',
  },
  leave: {
    title: 'Odchod zo servera',
    description: '**{user}** opustil server. Bolo nás {count}, teraz nás je {newCount}.',
  },

  // Admin commands
  admin: {
    configUpdated: '✅ Konfigurácia bola aktualizovaná.',
    roleCreated: '✅ Rola **{role}** bola vytvorená a pridaná do self-role systému.',
    roleDeleted: '✅ Rola **{role}** bola odobraná zo self-role systému.',
    noPermission: '❌ Nemáš oprávnenie na tento príkaz.',
    wordAdded: '✅ Slovo **{word}** bolo pridané do filtra.',
    wordRemoved: '✅ Slovo **{word}** bolo odobrané z filtra.',
  },

  // General
  general: {
    error: 'Vyskytla sa chyba. Skús to znova neskôr.',
    botName: 'CraftingBot',
  },
};
