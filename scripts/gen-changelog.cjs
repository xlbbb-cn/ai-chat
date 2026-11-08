#!/usr/bin/env node
/**
 * 把两个 ref（通常是相邻的两个 tag）之间的提交信息渲染成 changelog 段落。
 *
 * - 默认取「最新 tag」和它的上一个 tag 之间的提交；
 * - 按 Conventional Commits 的类型分组（固定顺序，与提交先后无关）；
 * - 每条提交带作用域、标题和指向 GitHub 提交页的短 SHA 链接；
 * - `--details` 额外带上提交正文里的要点（正文是中文时，条目就是中英对照）；
 * - `--write` 把段落插入/替换到 CHANGELOG.md 顶部（同一版本重复执行是幂等的）；
 * - `--output` 把正文写到文件，供 CI 填进 GitHub Release。
 *
 * 用法：
 *   node scripts/gen-changelog.cjs                       # 打印最新 tag 那一段
 *   node scripts/gen-changelog.cjs --from v2.9.0 --to v2.9.1 --details --write
 *   node scripts/gen-changelog.cjs --from v2.9.0 --to v2.9.1 --format body --output release-notes.md
 */
const fs = require('fs');
const path = require('path');
const { execFileSync } = require('child_process');

const root = path.resolve(__dirname, '..');
const changelogPath = path.join(root, 'CHANGELOG.md');

// 分组顺序是固定的：读者关心「新增」和「修复」，不该被提交时间决定先后。
const GROUPS = [
  ['feat', '新增'],
  ['fix', '修复'],
  ['perf', '性能'],
  ['refactor', '重构'],
  ['docs', '文档'],
  ['test', '测试'],
  ['build', '构建'],
  ['ci', 'CI'],
  ['chore', '杂项'],
];
const FALLBACK_GROUP = '其它';

// 版本号提交只改数字，对 changelog 没有信息量。
const SKIP_SUBJECTS = [/^chore\(release\)/i];

const FIELD = '\x1f'; // 记录内字段分隔（提交信息里不会出现）
const RECORD = '\x1e'; // 记录之间分隔

const CHANGELOG_HEADER = `# Changelog

本文件由 \`scripts/gen-changelog.cjs\` 从两个 tag 之间的提交信息生成，请勿手工编辑：

\`\`\`bash
npm run changelog -- --from <上一个 tag> --to <本次 tag> --details --write
\`\`\`

条目按 Conventional Commits 的类型分组，作用域加粗；工作区里提交标题是英文、正文是中文，
所以带 \`--details\` 时是中英对照。
`;

function git(...args) {
  return execFileSync('git', args, {
    cwd: root,
    encoding: 'utf8',
    stdio: ['pipe', 'pipe', 'ignore'],
  }).replace(/\s+$/, '');
}

function tryGit(...args) {
  try {
    return git(...args);
  } catch (error) {
    return '';
  }
}

function parseArgs() {
  const args = process.argv.slice(2);
  const opts = {
    from: null,
    to: null,
    format: 'file',
    write: false,
    output: null,
    details: false,
    help: false,
  };

  for (let i = 0; i < args.length; i += 1) {
    const arg = args[i];
    if (arg === '--from') {
      opts.from = args[i + 1];
      i += 1;
    } else if (arg === '--to') {
      opts.to = args[i + 1];
      i += 1;
    } else if (arg === '--format') {
      opts.format = args[i + 1];
      i += 1;
    } else if (arg === '--output' || arg === '-o') {
      opts.output = args[i + 1];
      i += 1;
    } else if (arg === '--write') {
      opts.write = true;
    } else if (arg === '--details') {
      opts.details = true;
    } else if (arg === '--help' || arg === '-h') {
      opts.help = true;
    } else {
      console.error(`未知参数：${arg}`);
      process.exit(1);
    }
  }

  return opts;
}

function printUsage() {
  console.log(`用法：node scripts/gen-changelog.cjs [选项]

  --from <ref>       起始 ref（不含），默认是 --to 的上一个 tag
  --to <ref>         结束 ref，默认是最新 tag
  --format <kind>    file（默认，带版本标题）| body（Release 正文）
  --details          附上提交正文里的要点
  --write            插入或替换到 CHANGELOG.md
  -o, --output <f>   把正文写到文件（CI 用）
  -h, --help         显示本帮助`);
}

function latestTag() {
  const tags = tryGit('tag', '--sort=-creatordate')
    .split(/\r?\n/)
    .filter(Boolean);
  return tags[0] || '';
}

function previousTag(ref) {
  if (!ref) return '';
  return tryGit('describe', '--tags', '--abbrev=0', `${ref}^`);
}

function isTagLike(ref) {
  return /^v?\d+\.\d+\.\d+/.test(ref);
}

/** 从 GitHub 的 remote 推导出网页地址，用于提交和对比链接；没有就返回空串。 */
function webUrl() {
  const url = tryGit('remote', 'get-url', 'github') || tryGit('remote', 'get-url', 'origin');
  if (!url) return '';
  const ssh = /^git@([^:]+):(.+?)(?:\.git)?$/.exec(url);
  if (ssh) return `https://${ssh[1]}/${ssh[2]}`;
  return url.replace(/\.git$/, '');
}

function commitLink(sha) {
  const short = sha.slice(0, 7);
  const base = webUrl();
  return base ? `[\`${short}\`](${base}/commit/${sha})` : `\`${short}\``;
}

function readCommits(range) {
  const raw = tryGit('log', '--no-merges', `--pretty=format:%H${FIELD}%s${FIELD}%b${RECORD}`, range);
  return raw
    .split(RECORD)
    .map((record) => record.replace(/^\r?\n/, ''))
    .filter((record) => record.trim())
    .map((record) => {
      const [sha, subject, body] = record.split(FIELD);
      return {
        sha: (sha || '').trim(),
        subject: (subject || '').trim(),
        body: (body || '').trim(),
      };
    })
    .filter((commit) => commit.subject && !SKIP_SUBJECTS.some((re) => re.test(commit.subject)));
}

function classify(subject) {
  const match = /^([a-z]+)(?:\(([^)]*)\))?!?:\s*(.+)$/i.exec(subject);
  if (!match) return { type: 'other', scope: '', message: subject };
  return { type: match[1].toLowerCase(), scope: match[2] || '', message: match[3].trim() };
}

function labelFor(type) {
  const found = GROUPS.find(([key]) => key === type);
  return found ? found[1] : FALLBACK_GROUP;
}

/**
 * 提交正文里的「- 要点」，用于 --details。
 *
 * 正文的列表项通常是折行的，逐行取会留下半句话（"…must open the line or"），
 * 所以把不属于新列表项的行拼回上一条，空行视为当前列表结束。
 */
function bodyBullets(body) {
  const bullets = [];
  let open = false;

  for (const raw of body.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line) {
      open = false;
      continue;
    }

    const bullet = /^[-*]\s+(.*)$/.exec(line);
    if (bullet) {
      bullets.push(bullet[1].trim());
      open = true;
    } else if (open) {
      bullets[bullets.length - 1] += ` ${line}`;
    }
  }

  return bullets;
}

function render(opts, base, target) {
  const commits = readCommits(base ? `${base}..${target}` : target);
  const date = tryGit('log', '-1', '--date=short', '--format=%ad', target) || '';
  const label = isTagLike(target) ? target : '未发布';

  const buckets = new Map();
  for (const commit of commits) {
    const { type, scope, message } = classify(commit.subject);
    const label_ = labelFor(type);
    const lines = buckets.get(label_) || [];
    lines.push(`- ${scope ? `**${scope}**: ` : ''}${message} (${commitLink(commit.sha)})`);
    if (opts.details) {
      for (const bullet of bodyBullets(commit.body)) {
        lines.push(`  - ${bullet}`);
      }
    }
    buckets.set(label_, lines);
  }

  const heading =
    opts.format === 'body'
      ? '## 本次更新'
      : // 只有真正的 tag 才有 release 页面可链；未发布的段落保持纯文本。
        `## ${
          webUrl() && isTagLike(target) ? `[${label}](${webUrl()}/releases/tag/${label})` : label
        }${date ? ` — ${date}` : ''}`;

  const out = [heading, ''];
  if (!commits.length) {
    out.push('无用户可见的变更。');
  } else {
    for (const [, group] of [...GROUPS, ['other', FALLBACK_GROUP]]) {
      const lines = buckets.get(group);
      if (!lines || !lines.length) continue;
      out.push(`### ${group}`, '', ...lines, '');
    }
  }

  if (base && isTagLike(target) && webUrl()) {
    out.push(`**完整对比**：[${base}...${target}](${webUrl()}/compare/${base}...${target})`, '');
  }

  return `${out.join('\n').replace(/\n{3,}/g, '\n\n').replace(/\s+$/, '')}\n`;
}

/** 把新段落插到第一个 `## ` 段落之前；同版本已存在则整段替换，保证可重复执行。 */
function upsert(text, section, label) {
  const parts = text.split(/\n(?=## )/);
  let header = parts.shift();
  if (!header || !header.trim()) header = CHANGELOG_HEADER;
  const sections = parts.map((part) => part.replace(/\s+$/, ''));
  const marker = `## [${label}]`;
  const plainMarker = `## ${label}`;
  const index = sections.findIndex(
    (part) => part.startsWith(marker) || part.startsWith(plainMarker),
  );
  if (index >= 0) {
    sections[index] = section.replace(/\s+$/, '');
  } else {
    sections.unshift(section.replace(/\s+$/, ''));
  }
  return `${header.replace(/\s+$/, '')}\n\n${sections.join('\n\n')}\n`;
}

function main() {
  const opts = parseArgs();
  if (opts.help) {
    printUsage();
    return;
  }

  if (opts.format !== 'file' && opts.format !== 'body') {
    console.error(`--format 只能是 file 或 body，收到：${opts.format}`);
    process.exit(1);
  }

  const target = opts.to || latestTag() || 'HEAD';
  const base = opts.from || previousTag(target);

  for (const ref of [base, target]) {
    if (ref && !tryGit('rev-parse', '--verify', '--quiet', `${ref}^{commit}`)) {
      console.error(`找不到 ref：${ref}`);
      process.exit(1);
    }
  }

  const label = isTagLike(target) ? target : '未发布';
  const section = render(opts, base, target);
  const commits = readCommits(base ? `${base}..${target}` : target).length;
  console.error(
    `${base ? `${base}..${target}` : target}：${commits} 个提交${opts.details ? '（含正文要点）' : ''}`,
  );

  if (opts.write) {
    const current = fs.existsSync(changelogPath)
      ? fs.readFileSync(changelogPath, 'utf8')
      : CHANGELOG_HEADER;
    const next = upsert(current, section, label);
    fs.writeFileSync(changelogPath, next, 'utf8');
    console.error(`CHANGELOG.md <- ${label}`);
  }

  if (opts.output) {
    const file = path.resolve(root, opts.output);
    fs.writeFileSync(file, section, 'utf8');
    console.error(`已写入 ${path.relative(root, file)}`);
    return;
  }

  process.stdout.write(section);
}

main();
