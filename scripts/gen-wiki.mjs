#!/usr/bin/env node
// 把 docs/wiki/{zh,en}/*.md 生成成 docs/site/wiki/ 下的静态页。
//
// 单一来源是仓库里的 markdown：站点页是产物，不入库（见 .gitignore），
// 和 docs/gen.py 生成 architecture.html 是同一套做法。
//   中文在 /wiki/（站点默认语言），英文在 /wiki/en/。
// 运行：node scripts/gen-wiki.mjs        （CI 里由 pages.yml 调用）

import { existsSync, statSync } from 'node:fs';
import { copyFile, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import MarkdownIt from 'markdown-it';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const SRC = path.join(ROOT, 'docs', 'wiki');
const OUT = path.join(ROOT, 'docs', 'site', 'wiki');
const SITE = 'https://xlbbb-cn.github.io/ai-chat';
const LANGS = ['zh', 'en'];

const nav = JSON.parse(await readFile(path.join(SRC, 'nav.json'), 'utf8'));
const order = nav.groups.flatMap((g) => g.pages);
const page = (slug) => nav.pages[slug];
const group = (slug) => nav.groups.find((g) => g.pages.includes(slug));

const UI = {
  zh: {
    home: '首页',
    docs: '文档',
    download: '下载',
    crumb: '文档',
    toc: '本页目录',
    prev: '上一篇',
    next: '下一篇',
    other: 'English',
    otherLabel: 'Switch to English',
    wiki: '本页同时维护在 GitHub Wiki',
    version: '文档版本',
    missing: '本页暂无中文版，以下为英文原文。',
    missingLink: '在 GitHub Wiki 上阅读英文版',
    theme: '切换主题',
    footerNote: '本站不加载第三方脚本与追踪代码。',
    license: 'MIT 许可证',
    privacy: '隐私政策',
    releases: '发布',
  },
  en: {
    home: 'Home',
    docs: 'Docs',
    download: 'Download',
    crumb: 'Docs',
    toc: 'On this page',
    prev: 'Previous',
    next: 'Next',
    other: '简体中文',
    otherLabel: '切换到中文',
    wiki: 'Maintained on the GitHub Wiki',
    version: 'Docs version',
    missing: '',
    missingLink: '',
    theme: 'Toggle theme',
    footerNote: 'No third-party scripts or trackers on this page.',
    license: 'MIT licensed',
    privacy: 'Privacy policy',
    releases: 'Releases',
  },
};

/* ---------- 路径与名称 ---------- */

const dirOf = (lang) => (lang === 'en' ? 'en/' : '');
const assetsOf = (lang) => (lang === 'en' ? '../../assets/' : '../assets/');
const siteOf = (lang) => (lang === 'en' ? '../../' : '../');
const outFile = (lang, slug) => path.join(OUT, dirOf(lang), `${slug}.html`);

/** 同一语言树内的链接；跨语言时给出对方树里的同一 slug。 */
const hrefIn = (from, to) => {
  if (from === to) return (slug) => `./${slug}.html`;
  if (to === 'en') return (slug) => `./en/${slug}.html`;
  return (slug) => `../${slug}.html`;
};

const norm = (s) =>
  String(s)
    .trim()
    .toLowerCase()
    .replace(/[\s_]+/g, '-')
    .replace(/\.(md|html)$/, '');

const alias = new Map();
for (const slug of order) {
  const p = page(slug);
  for (const key of [slug, p.wiki, p.en, p.zh, ...(p.aliases ?? [])]) {
    if (key) alias.set(norm(key), slug);
  }
}
const LANG_ALIAS = { 简体中文: 'zh', 简体中文版: 'zh', english: 'en' };

/* ---------- Markdown ---------- */

const md = new MarkdownIt({ html: true, linkify: true, typographer: false });

const stripMarkup = (s) => String(s).replace(/[`*_~]/g, '').replace(/\s+/g, ' ').trim();

// 与 GitHub 的标题锚点一致：小写、去标点、空格转连字符，保留中文。
const slugify = (s) =>
  String(s)
    .normalize('NFKC')
    .toLowerCase()
    .replace(/[`*~]/g, '')
    .replace(/[^\p{Letter}\p{Number}\s_-]/gu, '')
    .trim()
    .replace(/\s+/g, '-');

/** 去掉正文首行的语言切换行（站点自己的导航已经提供语言切换）。 */
function stripLanguageLine(source) {
  const lines = source.split(/\r?\n/);
  let i = 0;
  while (i < lines.length && !lines[i].trim()) i++;
  const first = lines[i] ?? '';
  if (!first.startsWith('#') && first.length < 240 && /\[English\]|\[简体中文\]|\[中文版/.test(first)) {
    lines.splice(i, 1);
    while (i < lines.length && !lines[i].trim()) lines.splice(i, 1);
  }
  return lines.join('\n').trim();
}

/** 取出并移除正文开头的一级标题，页面标题由外壳统一渲染。 */
function takeH1(source) {
  const m = /^#\s+(.+)$/m.exec(source);
  if (!m || m.index > 400) return { title: null, source };
  return { title: stripMarkup(m[1]), source: (source.slice(0, m.index) + source.slice(m.index + m[0].length)).trim() };
}

function renderPage(source) {
  const toc = [];
  const seen = new Map();
  md.renderer.rules.heading_open = (tokens, idx, options, env, self) => {
    const tok = tokens[idx];
    const level = Number(tok.tag.slice(1));
    const raw = tokens[idx + 1]?.content ?? '';
    const base = slugify(raw) || `section-${idx}`;
    const n = seen.get(base) ?? 0;
    seen.set(base, n + 1);
    const id = n ? `${base}-${n}` : base;
    tok.attrSet('id', id);
    if (level === 2 || level === 3) toc.push({ level, id, text: stripMarkup(raw) });
    return self.renderToken(tokens, idx, options);
  };
  return { body: md.render(source), toc };
}

const warnings = [];

const RAW_DOCS = /^https:\/\/raw\.githubusercontent\.com\/xlbbb-cn\/ai-chat\/main\/docs\/([A-Za-z0-9._-]+)$/;

/** 仓库 docs/ 里的图片复制进站点资产，页面只依赖 docs/site 自身。 */
async function selfHostMedia(html, lang) {
  const assets = assetsOf(lang);
  const need = [];
  const out = html.replace(/(src=")([^"]+)(")/g, (whole, pre, src, post) => {
    const m = RAW_DOCS.exec(src);
    if (!m) return whole;
    need.push(m[1]);
    return `${pre}${assets}${m[1]}${post}`;
  });
  for (const name of new Set(need)) {
    const from = path.join(ROOT, 'docs', name);
    const to = path.join(ROOT, 'docs', 'site', 'assets', name);
    if (existsSync(from) && !existsSync(to)) await copyFile(from, to);
  }
  return out;
}

/** 把 wiki 风格的页名链接改写成站内相对链接；解析不了的保持原样。 */
function rewriteLinks(html, lang) {
  const sameLangHref = hrefIn(lang, lang);
  return html.replace(/href="([^"]*)"/g, (whole, href) => {
    if (!href || href.startsWith('#') || /^(mailto|tel):/i.test(href)) return whole;
    const hashAt = href.indexOf('#');
    const anchor = hashAt >= 0 ? href.slice(hashAt) : '';
    const raw = hashAt >= 0 ? href.slice(0, hashAt) : href;
    if (!raw) return whole;

    const wikiUrl = /^https?:\/\/(?:www\.)?github\.com\/xlbbb-cn\/ai-chat\/wiki\/([^/?#]+)\/?$/i.exec(raw);
    let name;
    if (wikiUrl) name = decodeURIComponent(wikiUrl[1]);
    else if (/^[a-z]+:/i.test(raw)) return whole;
    else name = decodeURIComponent(raw.replace(/^\.\//, '')).replace(/^wiki\//, '');

    const key = norm(name);
    if (LANG_ALIAS[key]) {
      return `href="${hrefIn(lang, LANG_ALIAS[key])('index')}${anchor}"`;
    }
    const slug = alias.get(key);
    if (!slug) {
      if (wikiUrl) warnings.push(`未解析的 Wiki 链接：${href}`);
      return whole;
    }
    return `href="${sameLangHref(slug)}${anchor}"`;
  });
}

/* ---------- 外壳 ---------- */

const esc = (s) =>
  String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

function sidebar(lang, current) {
  const href = hrefIn(lang, lang);
  return nav.groups
    .map(
      (g) =>
        `<div class="wiki-group"><p class="wiki-group-title">${esc(g[lang])}</p><ul>` +
        g.pages
          .map((slug) => {
            const active = slug === current;
            return `<li><a href="${href(slug)}"${active ? ' class="is-active" aria-current="page"' : ''}>${esc(page(slug)[lang])}</a></li>`;
          })
          .join('') +
        '</ul></div>',
    )
    .join('');
}

function shell({ lang, slug, title, body, toc, prev, next, fallback }) {
  const t = UI[lang];
  const p = page(slug);
  const g = group(slug);
  const assets = assetsOf(lang);
  const site = siteOf(lang);
  const otherLang = lang === 'zh' ? 'en' : 'zh';
  const otherHref = hrefIn(lang, otherLang)(slug);
  const htmlLang = lang === 'zh' ? 'zh-CN' : 'en';
  const canonical = `${SITE}/wiki/${dirOf(lang)}${slug}.html`;

  const tocHtml = toc.length
    ? `<nav class="wiki-toc" aria-label="${t.toc}"><p class="wiki-toc-title">${t.toc}</p><ul>` +
      toc
        .map((h) => `<li class="lvl-${h.level}"><a href="#${h.id}">${esc(h.text)}</a></li>`)
        .join('') +
      '</ul></nav>'
    : '<nav class="wiki-toc" aria-hidden="true"></nav>';

  const pager =
    prev || next
      ? '<nav class="wiki-pager" aria-label="pager">' +
        (prev
          ? `<a class="wiki-prev" href="${hrefIn(lang, lang)(prev.slug)}"><span>← ${t.prev}</span><strong>${esc(prev.title)}</strong></a>`
          : '<span></span>') +
        (next
          ? `<a class="wiki-next" href="${hrefIn(lang, lang)(next.slug)}"><span>${t.next} →</span><strong>${esc(next.title)}</strong></a>`
          : '<span></span>') +
        '</nav>'
      : '';

  const notice = fallback
    ? `<p class="wiki-notice"><span>${t.missing}</span> <a href="https://github.com/${nav.repo}/wiki/${encodeURIComponent(p.wiki)}" target="_blank" rel="noopener noreferrer">${t.missingLink} ↗</a></p>`
    : '';

  return `<!DOCTYPE html>
<html lang="${htmlLang}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${esc(title)} · Rabbit Chat Studio ${t.crumb}</title>
<meta name="description" content="${esc(title)} — Rabbit Chat Studio ${t.crumb} (${esc(p[lang])}) · v${nav.docsVersion}">
<link rel="canonical" href="${canonical}">
<link rel="alternate" hreflang="zh-CN" href="${SITE}/wiki/${slug}.html">
<link rel="alternate" hreflang="en" href="${SITE}/wiki/en/${slug}.html">
<link rel="icon" href="${assets}favicon.png" sizes="256x256" type="image/png">
<link rel="apple-touch-icon" href="${assets}favicon.png">
<meta property="og:type" content="article">
<meta property="og:site_name" content="Rabbit Chat Studio">
<meta property="og:title" content="${esc(title)} · Rabbit Chat Studio">
<meta property="og:url" content="${canonical}">
<meta property="og:image" content="${SITE}/assets/og.jpg">
<meta name="twitter:card" content="summary_large_image">
<link rel="stylesheet" href="${assets}landing.css">
<link rel="stylesheet" href="${assets}wiki.css">
<script>
  try {
    var t = localStorage.getItem('rabbit-site-theme');
    if (t === 'light' || t === 'dark') document.documentElement.setAttribute('data-theme', t);
  } catch (e) {}
</script>
</head>
<body class="wiki-page">
<a class="skip" href="#main">${lang === 'zh' ? '跳到主要内容' : 'Skip to content'}</a>
<div class="page-glow" aria-hidden="true"></div>

<header class="nav-wrap">
  <div class="container nav">
    <a class="brand" href="${site}index.html" aria-label="Rabbit Chat Studio">
      <img src="${assets}rabbit-chat-studio-logo.png" alt="" width="42" height="42">
      <span>Rabbit Chat Studio</span>
    </a>
    <nav class="nav-links" aria-label="${t.crumb}">
      <a href="${site}index.html">${t.home}</a>
      <a href="./index.html" class="is-active">${t.docs}</a>
      <a href="${site}index.html#download">${t.download}</a>
    </nav>
    <div class="nav-actions">
      <button class="icon-button" id="theme-toggle" type="button" aria-label="${t.theme}" title="${t.theme}">
        <svg class="i-sun" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" aria-hidden="true"><circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M2 12h2M20 12h2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M19.1 4.9l-1.4 1.4M6.3 17.7l-1.4 1.4" stroke-linecap="round"/></svg>
        <svg class="i-moon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" aria-hidden="true"><path d="M21 14.5A8.5 8.5 0 0 1 9.5 3 8.5 8.5 0 1 0 21 14.5Z" stroke-linejoin="round"/></svg>
      </button>
      <a class="icon-button lang-button" href="${otherHref}" aria-label="${t.otherLabel}" title="${t.otherLabel}">${t.other === 'English' ? 'EN' : '中'}</a>
      <a class="button button-quiet nav-github" href="https://github.com/${nav.repo}" target="_blank" rel="noopener noreferrer">
        <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M12 2a10 10 0 0 0-3.16 19.49c.5.09.68-.22.68-.48v-1.7c-2.78.6-3.37-1.34-3.37-1.34-.45-1.15-1.11-1.46-1.11-1.46-.91-.62.07-.61.07-.61 1 .07 1.53 1.03 1.53 1.03.9 1.53 2.36 1.09 2.94.84.09-.65.35-1.09.63-1.34-2.22-.25-4.56-1.11-4.56-4.94 0-1.09.39-1.98 1.03-2.68-.1-.25-.45-1.27.1-2.65 0 0 .84-.27 2.75 1.02A9.6 9.6 0 0 1 12 6.8c.85 0 1.7.11 2.5.33 1.9-1.29 2.74-1.02 2.74-1.02.55 1.38.2 2.4.1 2.65.64.7 1.03 1.59 1.03 2.68 0 3.84-2.34 4.68-4.57 4.93.36.31.68.92.68 1.86v2.76c0 .27.18.58.69.48A10 10 0 0 0 12 2Z"/></svg>
        <span>GitHub</span>
      </a>
    </div>
  </div>
</header>

<div class="container wiki-shell">
  <aside class="wiki-side">
    <details class="wiki-side-box" open>
      <summary>${t.crumb} · v${nav.docsVersion}</summary>
      ${sidebar(lang, slug)}
    </details>
  </aside>

  <main id="main" class="wiki-main">
    <p class="wiki-crumb"><a href="./index.html">${t.crumb}</a><span aria-hidden="true">/</span>${esc(g[lang])}</p>
    <h1>${esc(title)}</h1>
    ${notice}
    <article class="prose">
${body}
    </article>
    ${pager}
    <p class="wiki-foot">
      ${t.wiki}：<a href="https://github.com/${nav.repo}/wiki/${encodeURIComponent(p.wiki)}" target="_blank" rel="noopener noreferrer">${esc(p.wiki)} ↗</a>
      · ${t.version} v${nav.docsVersion}
    </p>
  </main>

  ${tocHtml}
</div>

<footer class="site-footer">
  <div class="container footer-inner">
    <a class="footer-brand" href="${site}index.html">
      <img src="${assets}rabbit-chat-studio-logo.png" alt="" width="29" height="29">
      <span>Rabbit Chat Studio</span>
      <span class="footnote">${t.license}</span>
    </a>
    <nav class="footer-links" aria-label="footer">
      <a href="https://github.com/${nav.repo}" target="_blank" rel="noopener noreferrer">GitHub</a>
      <a href="https://github.com/${nav.repo}/releases" target="_blank" rel="noopener noreferrer">${t.releases}</a>
      <a href="https://github.com/${nav.repo}/wiki" target="_blank" rel="noopener noreferrer">Wiki</a>
      <a href="${site}privacy-policy.html">${t.privacy}</a>
    </nav>
    <p class="footnote footer-note">${t.footerNote}</p>
  </div>
</footer>

<script>
  (function () {
    var nav = document.querySelector('.wiki-side-box');
    if (!nav || nav.tagName !== 'DETAILS') return;
    var mq = matchMedia('(max-width: 900px)');
    function sync() { nav.open = !mq.matches; }
    sync();
    if (mq.addEventListener) mq.addEventListener('change', sync);
  })();
  (function () {
    var button = document.getElementById('theme-toggle');
    if (!button) return;
    button.addEventListener('click', function () {
      var current = document.documentElement.getAttribute('data-theme');
      var dark = current ? current === 'dark' : matchMedia('(prefers-color-scheme: dark)').matches;
      var next = dark ? 'light' : 'dark';
      document.documentElement.setAttribute('data-theme', next);
      try { localStorage.setItem('rabbit-site-theme', next); } catch (e) {}
    });
  })();
</script>
</body>
</html>
`;
}

/* ---------- 生成 ---------- */

const written = new Map(); // 绝对路径 -> 该页的锚点集合
const missing = [];

await rm(OUT, { recursive: true, force: true });

for (const lang of LANGS) {
  for (const [index, slug] of order.entries()) {
    const target = outFile(lang, slug);
    let source;
    let fallback = false;
    try {
      source = await readFile(path.join(SRC, lang, `${slug}.md`), 'utf8');
    } catch {
      if (lang !== 'zh') {
        missing.push(`docs/wiki/en/${slug}.md`);
        continue;
      }
      const enPath = path.join(SRC, 'en', `${slug}.md`);
      if (!existsSync(enPath)) {
        missing.push(`docs/wiki/{zh,en}/${slug}.md`);
        continue;
      }
      source = await readFile(enPath, 'utf8');
      fallback = true;
    }

    const stripped = stripLanguageLine(source);
    const { title: h1, source: bodySource } = takeH1(stripped);
    const { body, toc } = renderPage(bodySource);
    const title = h1 ?? page(slug)[lang];

    const prevSlug = order[index - 1];
    const nextSlug = order[index + 1];
    const prev = prevSlug ? { slug: prevSlug, title: page(prevSlug)[lang] } : null;
    const next = nextSlug ? { slug: nextSlug, title: page(nextSlug)[lang] } : null;

    const html = shell({
      lang,
      slug,
      title,
      body: await selfHostMedia(rewriteLinks(body, lang), lang),
      toc,
      prev,
      next,
      fallback,
    });

    await mkdir(path.dirname(target), { recursive: true });
    await writeFile(target, html, 'utf8');
    written.set(target, new Set(toc.map((h) => h.id)));
  }
}

/* ---------- 断链自检 ---------- */

const problems = [];
for (const [file, anchors] of written) {
  const html = await readFile(file, 'utf8');
  for (const match of html.matchAll(/(?:href|src)="([^"]*)"/g)) {
    const href = match[1];
    if (!href || href.startsWith('#') || /^[a-z]+:/i.test(href)) continue;
    const [relTarget, anchor] = href.split('#');
    const abs = path.resolve(path.dirname(file), relTarget || path.basename(file));
    const relative = path.relative(ROOT, abs).replace(/\\/g, '/');
    if (written.has(abs)) {
      if (anchor && !written.get(abs).has(decodeURIComponent(anchor))) {
        problems.push(`${path.relative(ROOT, file)} → ${href}：锚点不存在`);
      }
      continue;
    }
    if (!existsSync(abs) || !statSync(abs).isFile()) {
      problems.push(`${path.relative(ROOT, file)} → ${href}：目标不存在`);
      continue;
    }
    if (!relative.startsWith('docs/site/')) {
      problems.push(`${path.relative(ROOT, file)} → ${href}：链接到站点之外`);
    }
  }
}

const pages = written.size;
const zhPages = [...written.keys()].filter((f) => !f.includes(`${path.sep}en${path.sep}`)).length;

for (const w of [...new Set(warnings)]) console.warn(`warn: ${w}`);

if (missing.length) {
  console.error(`缺少源文件（${missing.length}）：\n  ${missing.join('\n  ')}`);
}
if (problems.length) {
  console.error(`断链（${problems.length}）：\n  ${problems.join('\n  ')}`);
}
if (problems.length || missing.length) process.exit(1);

console.log(`gen-wiki: 生成 ${pages} 页（中文 ${zhPages} / 英文 ${pages - zhPages}），文档版本 v${nav.docsVersion}，无断链。`);