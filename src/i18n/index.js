const sk = require('./sk');
const en = require('./en');

const languages = { sk, en };

function t(key, lang = process.env.DEFAULT_LANGUAGE || 'sk', replacements = {}) {
  const keys = key.split('.');
  let value = languages[lang];
  for (const k of keys) {
    value = value?.[k];
  }
  if (!value) {
    // Fallback to Slovak
    value = languages.sk;
    for (const k of keys) {
      value = value?.[k];
    }
  }
  if (typeof value !== 'string') return key;

  return value.replace(/\{(\w+)\}/g, (_, name) => replacements[name] ?? `{${name}}`);
}

module.exports = { t, languages };
