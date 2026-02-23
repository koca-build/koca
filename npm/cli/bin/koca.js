#!/usr/bin/env node

const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const binName = process.platform === 'win32' ? 'koca.exe' : 'koca';
const localBin = path.join(__dirname, binName);

if (!fs.existsSync(localBin)) {
  console.error('koca binary is missing. Reinstall @koca-build/cli to download the release binary.');
  process.exit(1);
}

const result = spawnSync(localBin, process.argv.slice(2), { stdio: 'inherit' });

if (result.error) {
  console.error(result.error.message);
  process.exit(1);
}

process.exit(result.status ?? 1);
