#!/usr/bin/env node

const fs = require('node:fs');
const path = require('node:path');
const https = require('node:https');

const pkg = require('../package.json');

const supported = process.platform === 'linux' && process.arch === 'x64';
if (!supported) {
  console.error(`@koca-build/cli currently supports linux-x64 for binary install. Detected ${process.platform}-${process.arch}.`);
  process.exit(1);
}

const outPath = path.join(__dirname, '..', 'bin', 'koca');
const url = `https://github.com/koca-build/koca/releases/download/v${pkg.version}/koca-linux-x64`;

fs.mkdirSync(path.dirname(outPath), { recursive: true });

download(url);

function download(downloadUrl) {
  https
    .get(downloadUrl, (res) => {
      if (res.statusCode && res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        download(res.headers.location);
        return;
      }

      if (res.statusCode !== 200) {
        console.error(`Failed to download koca binary from ${downloadUrl} (status ${res.statusCode}).`);
        process.exit(1);
      }

      const file = fs.createWriteStream(outPath, { mode: 0o755 });
      res.pipe(file);

      file.on('finish', () => {
        file.close(() => {
          fs.chmodSync(outPath, 0o755);
        });
      });

      file.on('error', (err) => {
        console.error(`Failed to write koca binary: ${err.message}`);
        process.exit(1);
      });
    })
    .on('error', (err) => {
      console.error(`Failed to download koca binary: ${err.message}`);
      process.exit(1);
    });
}
