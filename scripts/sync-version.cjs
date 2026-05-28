#!/usr/bin/env node
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const root = path.resolve(__dirname, '..');
const pkgPath = path.join(root, 'package.json');
const cargoPath = path.join(root, 'src-tauri', 'Cargo.toml');
const tauriPath = path.join(root, 'src-tauri', 'tauri.conf.json');

function parseArgs() {
  const args = process.argv.slice(2);
  const opts = { version: null, fromGitTag: false, bump: null, dryRun: false };

  for (let i = 0; i < args.length; i += 1) {
    const arg = args[i];
    if (arg === '--from-git-tag') {
      opts.fromGitTag = true;
    } else if (arg === '--dry-run') {
      opts.dryRun = true;
    } else if (arg === '--version' || arg === '-v') {
      opts.version = args[i + 1];
      i += 1;
    } else if (arg === '--bump') {
      opts.bump = args[i + 1];
      i += 1;
    }
  }

  return opts;
}

function normalizeVersion(version) {
  if (!version) return null;
  return version.trim().replace(/^v/i, '');
}

function readJson(filePath) {
  return JSON.parse(fs.readFileSync(filePath, 'utf8'));
}

function writeJson(filePath, data) {
  fs.writeFileSync(filePath, JSON.stringify(data, null, 2) + '\n', 'utf8');
}

function bumpVersion(version, type) {
  const parts = version.split('.').map((part) => parseInt(part, 10));
  if (parts.length !== 3 || parts.some(Number.isNaN)) {
    throw new Error(`当前版本不是有效 semver: ${version}`);
  }

  const [major, minor, patch] = parts;
  if (type === 'patch') {
    return `${major}.${minor}.${patch + 1}`;
  }
  if (type === 'minor') {
    return `${major}.${minor + 1}.0`;
  }
  if (type === 'major') {
    return `${major + 1}.0.0`;
  }
  throw new Error(`未知的 bump 类型: ${type}`);
}

function getLatestGitTag() {
  try {
    const tags = execSync('git tag --sort=-creatordate', {
      cwd: root,
      encoding: 'utf8',
      stdio: ['pipe', 'pipe', 'ignore'],
    })
      .trim()
      .split(/\r?\n/)
      .filter(Boolean);

    if (tags.length > 0) {
      return normalizeVersion(tags[0]);
    }

    const fallback = execSync('git describe --tags --abbrev=0', {
      cwd: root,
      encoding: 'utf8',
      stdio: ['pipe', 'pipe', 'ignore'],
    }).trim();
    return normalizeVersion(fallback);
  } catch (error) {
    throw new Error('读取 git tag 失败，请确认仓库中存在标签。');
  }
}

function updateFile(filePath, transform) {
  const original = fs.readFileSync(filePath, 'utf8');
  const updated = transform(original);
  if (original !== updated) {
    fs.writeFileSync(filePath, updated, 'utf8');
    return true;
  }
  return false;
}

function updateCargoToml(version) {
  return updateFile(cargoPath, (content) =>
    content.replace(/^version\s*=\s*"[^"]+"/m, `version = "${version}"`),
  );
}

function updateTauriConf(version) {
  return updateFile(tauriPath, (content) =>
    content.replace(/"version"\s*:\s*"[^"]+"/, `"version": "${version}"`),
  );
}

function updatePackageJson(version) {
  const pkg = readJson(pkgPath);
  pkg.version = version;
  if (!process.argv.includes('--dry-run')) {
    writeJson(pkgPath, pkg);
  }
  return pkg.version;
}

function main() {
  const opts = parseArgs();
  let version = null;

  if (opts.fromGitTag) {
    version = getLatestGitTag();
  } else if (opts.version) {
    version = normalizeVersion(opts.version);
  } else {
    const pkg = readJson(pkgPath);
    version = pkg.version;
  }

  if (!version) {
    console.error('必须通过 --version 或 --from-git-tag 指定版本，或者在 package.json 中已有 version。');
    process.exit(1);
  }

  if (opts.bump) {
    version = bumpVersion(version, opts.bump);
  }

  console.log(`同步版本到: ${version}`);
  if (opts.dryRun) {
    process.exit(0);
  }

  updatePackageJson(version);
  const cargoUpdated = updateCargoToml(version);
  const tauriUpdated = updateTauriConf(version);

  console.log(`package.json -> ${version}`);
  console.log(`src-tauri/Cargo.toml -> ${cargoUpdated ? '已更新' : '保持不变'}`);
  console.log(`src-tauri/tauri.conf.json -> ${tauriUpdated ? '已更新' : '保持不变'}`);
}

main();
