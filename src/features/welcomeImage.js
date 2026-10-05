const { createCanvas, loadImage, GlobalFonts } = require('@napi-rs/canvas');
const { AttachmentBuilder } = require('discord.js');
const path = require('path');
const https = require('https');
const http = require('http');

const CANVAS_WIDTH = 1024;
const CANVAS_HEIGHT = 380;

// Colors matching the crafting.sk brand
const COLORS = {
  bgDark: '#1a1a2e',
  bgMid: '#16213e',
  bgLight: '#0f3460',
  accent: '#6b8e23',       // olive green from logo
  accentLight: '#8fbc3b',  // lighter green
  accentGlow: '#a4d65e',   // bright green
  textWhite: '#ffffff',
  textGray: '#b0b0b0',
  textDim: '#666666',
  red: '#e74c3c',
};

const AVATAR_X = 220;
const AVATAR_Y = 190;
const AVATAR_SIZE = 180;
const TEXT_X = 620;

function fetchImage(url) {
  return new Promise((resolve, reject) => {
    const client = url.startsWith('https') ? https : http;
    client.get(url, (res) => {
      const chunks = [];
      res.on('data', (chunk) => chunks.push(chunk));
      res.on('end', () => resolve(Buffer.concat(chunks)));
      res.on('error', reject);
    }).on('error', reject);
  });
}

function drawRoundedRect(ctx, x, y, w, h, r) {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.lineTo(x + w - r, y);
  ctx.quadraticCurveTo(x + w, y, x + w, y + r);
  ctx.lineTo(x + w, y + h - r);
  ctx.quadraticCurveTo(x + w, y + h, x + w - r, y + h);
  ctx.lineTo(x + r, y + h);
  ctx.quadraticCurveTo(x, y + h, x, y + h - r);
  ctx.lineTo(x, y + r);
  ctx.quadraticCurveTo(x, y, x + r, y);
  ctx.closePath();
}

function drawBackground(ctx, type) {
  // Dark gradient background
  const gradient = ctx.createLinearGradient(0, 0, CANVAS_WIDTH, CANVAS_HEIGHT);
  gradient.addColorStop(0, COLORS.bgDark);
  gradient.addColorStop(0.5, COLORS.bgMid);
  gradient.addColorStop(1, COLORS.bgLight);
  ctx.fillStyle = gradient;
  ctx.fillRect(0, 0, CANVAS_WIDTH, CANVAS_HEIGHT);

  // Subtle grid pattern (Minecraft-inspired)
  ctx.strokeStyle = 'rgba(107, 142, 35, 0.06)';
  ctx.lineWidth = 1;
  for (let x = 0; x < CANVAS_WIDTH; x += 32) {
    ctx.beginPath();
    ctx.moveTo(x, 0);
    ctx.lineTo(x, CANVAS_HEIGHT);
    ctx.stroke();
  }
  for (let y = 0; y < CANVAS_HEIGHT; y += 32) {
    ctx.beginPath();
    ctx.moveTo(0, y);
    ctx.lineTo(CANVAS_WIDTH, y);
    ctx.stroke();
  }

  // Decorative corner blocks (Minecraft dirt block style)
  const blockSize = 16;
  const blockColor = 'rgba(107, 142, 35, 0.12)';
  ctx.fillStyle = blockColor;

  // Top-left corner blocks
  for (let i = 0; i < 5; i++) {
    for (let j = 0; j < 5 - i; j++) {
      ctx.fillRect(i * blockSize, j * blockSize, blockSize - 1, blockSize - 1);
    }
  }

  // Bottom-right corner blocks
  for (let i = 0; i < 5; i++) {
    for (let j = 0; j < 5 - i; j++) {
      ctx.fillRect(
        CANVAS_WIDTH - (i + 1) * blockSize,
        CANVAS_HEIGHT - (j + 1) * blockSize,
        blockSize - 1,
        blockSize - 1,
      );
    }
  }

  // Top-right corner blocks
  for (let i = 0; i < 3; i++) {
    for (let j = 0; j < 3 - i; j++) {
      ctx.fillRect(CANVAS_WIDTH - (i + 1) * blockSize, j * blockSize, blockSize - 1, blockSize - 1);
    }
  }

  // Accent line at top
  const accentGrad = ctx.createLinearGradient(0, 0, CANVAS_WIDTH, 0);
  accentGrad.addColorStop(0, 'rgba(107, 142, 35, 0)');
  accentGrad.addColorStop(0.3, COLORS.accent);
  accentGrad.addColorStop(0.7, COLORS.accentLight);
  accentGrad.addColorStop(1, 'rgba(107, 142, 35, 0)');
  ctx.fillStyle = accentGrad;
  ctx.fillRect(0, 0, CANVAS_WIDTH, 4);

  // Bottom accent line
  ctx.fillStyle = accentGrad;
  ctx.fillRect(0, CANVAS_HEIGHT - 4, CANVAS_WIDTH, 4);

  // Vertical separator line between avatar and text
  const sepGrad = ctx.createLinearGradient(0, 40, 0, CANVAS_HEIGHT - 40);
  const sepColor = type === 'leave' ? 'rgba(231, 76, 60, 0.15)' : 'rgba(107, 142, 35, 0.15)';
  sepGrad.addColorStop(0, 'transparent');
  sepGrad.addColorStop(0.5, sepColor);
  sepGrad.addColorStop(1, 'transparent');
  ctx.fillStyle = sepGrad;
  ctx.fillRect(410, 40, 2, CANVAS_HEIGHT - 80);

  // Glow behind avatar area
  const glowColor = type === 'leave' ? 'rgba(231, 76, 60, 0.08)' : 'rgba(107, 142, 35, 0.08)';
  const glowGrad = ctx.createRadialGradient(
    AVATAR_X, AVATAR_Y, 10,
    AVATAR_X, AVATAR_Y, 200,
  );
  glowGrad.addColorStop(0, glowColor);
  glowGrad.addColorStop(1, 'transparent');
  ctx.fillStyle = glowGrad;
  ctx.fillRect(0, 0, CANVAS_WIDTH, CANVAS_HEIGHT);
}

function drawAvatar(ctx, avatarImage, x, y, size, type) {
  const borderColor = type === 'leave' ? COLORS.red : COLORS.accentLight;

  // Outer glow
  ctx.save();
  ctx.shadowColor = borderColor;
  ctx.shadowBlur = 25;
  ctx.beginPath();
  ctx.arc(x, y, size / 2 + 5, 0, Math.PI * 2);
  ctx.fillStyle = borderColor;
  ctx.fill();
  ctx.restore();

  // Border ring
  ctx.beginPath();
  ctx.arc(x, y, size / 2 + 5, 0, Math.PI * 2);
  ctx.fillStyle = borderColor;
  ctx.fill();

  // Dark ring
  ctx.beginPath();
  ctx.arc(x, y, size / 2 + 2, 0, Math.PI * 2);
  ctx.fillStyle = COLORS.bgDark;
  ctx.fill();

  // Clip and draw avatar
  ctx.save();
  ctx.beginPath();
  ctx.arc(x, y, size / 2, 0, Math.PI * 2);
  ctx.closePath();
  ctx.clip();
  ctx.drawImage(avatarImage, x - size / 2, y - size / 2, size, size);
  ctx.restore();
}

async function drawLogo(ctx, x, y, maxHeight) {
  try {
    const logoPath = path.join(__dirname, '..', '..', 'crafting-logo-small.png');
    const logo = await loadImage(logoPath);
    const ratio = logo.width / logo.height;
    const height = maxHeight;
    const width = height * ratio;
    ctx.globalAlpha = 0.7;
    ctx.drawImage(logo, x, y, width, height);
    ctx.globalAlpha = 1;
    return width;
  } catch {
    return 0;
  }
}

async function generateWelcomeImage(member) {
  const canvas = createCanvas(CANVAS_WIDTH, CANVAS_HEIGHT);
  const ctx = canvas.getContext('2d');

  // Background
  drawBackground(ctx, 'welcome');

  // Load and draw avatar on the left
  const avatarURL = member.user.displayAvatarURL({ extension: 'png', size: 256 });
  try {
    const avatarBuffer = await fetchImage(avatarURL);
    const avatarImage = await loadImage(avatarBuffer);
    drawAvatar(ctx, avatarImage, AVATAR_X, AVATAR_Y, AVATAR_SIZE, 'welcome');
  } catch {
    ctx.beginPath();
    ctx.arc(AVATAR_X, AVATAR_Y, AVATAR_SIZE / 2, 0, Math.PI * 2);
    ctx.fillStyle = COLORS.accent;
    ctx.fill();
  }

  // "WELCOME" title on the right
  ctx.textAlign = 'center';
  ctx.fillStyle = COLORS.accentGlow;
  ctx.font = '600 46px "Segoe UI", "Arial", sans-serif';
  ctx.fillText('W E L C O M E', TEXT_X, 130);

  // Username
  ctx.fillStyle = COLORS.textWhite;
  ctx.font = '700 36px "Segoe UI", "Arial", sans-serif';
  const username = member.user.globalName || member.user.username;
  const displayName = username.length > 25 ? username.substring(0, 22) + '...' : username;
  ctx.fillText(displayName, TEXT_X, 185);

  // Server name (lowercase)
  ctx.fillStyle = COLORS.textGray;
  ctx.font = '400 22px "Segoe UI", "Arial", sans-serif';
  ctx.fillText('crafting.sk', TEXT_X, 225);

  // Member count badge
  const countText = `#${member.guild.memberCount}`;
  ctx.font = '600 18px "Segoe UI", "Arial", sans-serif';
  const countWidth = ctx.measureText(countText).width;

  drawRoundedRect(ctx, TEXT_X - countWidth / 2 - 16, 248, countWidth + 32, 32, 16);
  ctx.fillStyle = 'rgba(107, 142, 35, 0.2)';
  ctx.fill();
  ctx.strokeStyle = COLORS.accent;
  ctx.lineWidth = 1;
  ctx.stroke();

  ctx.fillStyle = COLORS.accentLight;
  ctx.textAlign = 'center';
  ctx.fillText(countText, TEXT_X, 270);

  // Logo in bottom-right
  await drawLogo(ctx, CANVAS_WIDTH - 100, CANVAS_HEIGHT - 55, 40);

  const buffer = canvas.toBuffer('image/png');
  return new AttachmentBuilder(buffer, { name: 'welcome.png' });
}

async function generateLeaveImage(member) {
  const canvas = createCanvas(CANVAS_WIDTH, CANVAS_HEIGHT);
  const ctx = canvas.getContext('2d');

  // Background
  drawBackground(ctx, 'leave');

  // Load and draw avatar on the left
  const avatarURL = member.user.displayAvatarURL({ extension: 'png', size: 256 });
  try {
    const avatarBuffer = await fetchImage(avatarURL);
    const avatarImage = await loadImage(avatarBuffer);
    drawAvatar(ctx, avatarImage, AVATAR_X, AVATAR_Y, AVATAR_SIZE, 'leave');
  } catch {
    ctx.beginPath();
    ctx.arc(AVATAR_X, AVATAR_Y, AVATAR_SIZE / 2, 0, Math.PI * 2);
    ctx.fillStyle = COLORS.red;
    ctx.fill();
  }

  // "GOODBYE" title on the right
  ctx.textAlign = 'center';
  ctx.fillStyle = COLORS.red;
  ctx.font = '600 46px "Segoe UI", "Arial", sans-serif';
  ctx.fillText('G O O D B Y E', TEXT_X, 130);

  // Username
  ctx.fillStyle = COLORS.textGray;
  ctx.font = '700 36px "Segoe UI", "Arial", sans-serif';
  const username = member.user.globalName || member.user.username;
  const displayName = username.length > 25 ? username.substring(0, 22) + '...' : username;
  ctx.fillText(displayName, TEXT_X, 185);

  // Server name (lowercase)
  ctx.fillStyle = COLORS.textDim;
  ctx.font = '400 22px "Segoe UI", "Arial", sans-serif';
  ctx.fillText('crafting.sk', TEXT_X, 225);

  // Remaining count badge
  const countText = `${member.guild.memberCount} members`;
  ctx.font = '600 18px "Segoe UI", "Arial", sans-serif';
  const countWidth = ctx.measureText(countText).width;

  drawRoundedRect(ctx, TEXT_X - countWidth / 2 - 16, 248, countWidth + 32, 32, 16);
  ctx.fillStyle = 'rgba(231, 76, 60, 0.15)';
  ctx.fill();
  ctx.strokeStyle = COLORS.red;
  ctx.lineWidth = 1;
  ctx.stroke();

  ctx.fillStyle = COLORS.red;
  ctx.textAlign = 'center';
  ctx.fillText(countText, TEXT_X, 270);

  // Logo in bottom-right
  await drawLogo(ctx, CANVAS_WIDTH - 100, CANVAS_HEIGHT - 55, 40);

  const buffer = canvas.toBuffer('image/png');
  return new AttachmentBuilder(buffer, { name: 'goodbye.png' });
}

module.exports = { generateWelcomeImage, generateLeaveImage };
