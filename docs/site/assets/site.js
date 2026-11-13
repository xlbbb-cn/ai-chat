/* Rabbit Chat Studio — landing page behaviour.
   Theme, language, OS detection and copy buttons. No dependencies. */
(function () {
  'use strict';

  var EN = {
    'a11y.skip': 'Skip to content',
    'nav.capabilities': 'Capabilities',
    'nav.compare': 'Compare',
    'nav.security': 'Boundaries',
    'nav.start': 'Start',
    'nav.docs': 'Docs',

    'hero.eyebrow': 'Tauri 2 · MIT · local-first',
    'hero.title': 'Turn any OpenAI-compatible model into a desktop agent that knows its workspace.',
    'hero.lede': 'A skills sandbox, self-evolving memory, sub-agent orchestration and MCP, ready on first launch. Config, history and every request stay on this machine — no sign-up, no account, no hidden prompt rewriting.',
    'hero.download': 'Download for Windows',
    'hero.allPlatforms': 'All platforms',
    'hero.repo': 'View on GitHub',
    'hero.fine': 'Windows installer or Microsoft Store · macOS (Apple Silicon / Intel) · Linux AppImage. Point it at an API key or a local model and you are done.',

    'mock.user': 'Run the tests, fix only the failing one. Touch nothing else.',
    'mock.reply': 'Reading the workspace AGENTS.md first, then running the tests.',
    'mock.model': 'deepseek-v4 · 12 tools · evolution memory loaded',
    'mock.state1': 'done · 4.2s',
    'mock.state2': 'running in background',
    'mock.placeholder': 'Message, press Enter to send…',
    'mock.send': 'Send',

    'cap.title': 'What it does',
    'cap.lede': 'Chat is only the entry point. The agent gets a workspace, tools that actually run, and an audit trail you can read.',
    'cap.skills.title': 'Skills sandbox',
    'cap.skills.body': 'Each workspace has exactly one skills root, <code>.data/skills</code>, and every skill is locked to its own SKILL.md. Enablement is stored per workspace too.',
    'cap.evo.title': 'Self-evolution',
    'cap.evo.body': 'The assistant installs, edits and deletes its own skills, and writes verified facts into evolution memory. Every change leaves a <code>.bak.n</code> backup.',
    'cap.agents.title': 'Sub-agent orchestration',
    'cap.agents.body': 'Three runtimes — llm, cli, cloud — with file-based definitions at <code>agents/&lt;category&gt;/&lt;slug&gt;.md</code>. Model, tool allowlist and MCP are configured per agent.',
    'cap.tools.title': 'Built-in tools',
    'cap.tools.body': 'Four memory scopes, todos, one-shot timers, file operations and patches. <code>run_cmd</code> runs a PATH program; <code>run_shell</code> scripts pass the risk engine first.',
    'cap.mcp.title': 'MCP',
    'cap.mcp.body': 'stdio, SSE, HTTP and Streamable HTTP, with a connectivity test. Each sub-agent task keeps its own server process and exits when the task ends.',
    'cap.audit.title': 'Local audit and background runs',
    'cap.audit.body': 'Request body, tokens, latency, status and error go into local SQLite. Slow tools, MCP calls and sub-agents can run in the background while the model keeps going.',

    'cmp.title': 'How it is different',
    'cmp.lede': 'Same chat box, different contract: does the agent have a workspace, a permission boundary and a replayable record?',
    'cmp.col.cap': 'Capability',
    'cmp.r1': 'Skills sandbox · single skills root',
    'cmp.r2': 'Self-evolution · installs skills, keeps its own memory',
    'cmp.r3': 'Built-in agent tools · memory, timers, todos',
    'cmp.r4': 'No account required',
    'cmp.r5': 'Prompt ownership · no default persona',
    'cmp.r6': 'Local request audit',
    'cmp.r7': 'Background runs · slow tools do not block the turn',
    'cmp.note': '✓ native · ◐ partial or adjacent · — not offered. Based on public documentation as of November 2026.',

    'sec.title': 'Capability needs a written boundary',
    'sec.lede': 'The three execution tools deliberately have different reach, and the one path that escapes the workspace is gated separately.',
    'sec.col.tool': 'Tool',
    'sec.col.scope': 'Reach',
    'sec.col.guard': 'How it is constrained',
    'sec.f.scope': 'Inside the workspace root, including the hidden .data (except the workspace database)',
    'sec.f.guard': 'Path sandbox: outside paths, ../ escapes and symlink escapes are all rejected',
    'sec.c.scope': 'One program that the system resolves through PATH',
    'sec.c.guard': 'Name validation, risk levels L0–L6, L0–L3 require confirmation',
    'sec.s.scope': 'Anywhere, including outside the workspace',
    'sec.s.guard': 'Risk engine and syntax gate before each script, L0–L3 require confirmation',
    'sec.note': 'Self-evolution lets the assistant rewrite its own instructions: keep it in a controlled workspace, keep it short, and review the .bak backups afterwards. MCP tool arguments are forwarded as-is — verify least privilege before connecting an external service.',

    'start.title': 'Three steps',
    'start.s1.title': 'Install the app',
    'start.s1.body': 'Download the installer for your platform. Windows ships an NSIS installer; macOS gets separate Apple Silicon and Intel images.',
    'start.s2.title': 'Connect a model',
    'start.s2.body': 'An OpenAI-compatible base URL plus an API key, or a local gateway. No account, no forced sign-in.',
    'start.s3.title': 'Pick a workspace',
    'start.s3.body': 'The workspace is the agent\u2019s reach: skills, memory, todos and the database all live in its .data directory.',

    'dl.title': 'Download',
    'dl.version': 'Latest release:',
    'dl.win': 'x64 · NSIS installer',
    'dl.store': 'Windows 10 2004+ · installed and updated by the Store',
    'dl.macArm': 'M-series chips',
    'dl.macIntel': 'Intel chips',
    'dl.linux': 'x86_64 · AppImage',
    'dl.pkgTitle': 'Open the Store page',
    'dl.pkgNote': 'Paste this into the Run box (Win+R) or your address bar to open the Store page. winget, Homebrew and Scoop manifests are not available yet.',
    'dl.copy': 'Copy',
    'dl.devTitle': 'Build from source',
    'dl.devReq': 'Requires Node.js 20.19+ (or 22.12+) and a stable Rust toolchain; Windows should use the MSVC toolchain.',

    'docs.title': 'Documentation',
    'docs.lede': 'From install to configuration, skills, MCP and sub-agents — the Wiki carries the full guide, with a complete Simplified Chinese mirror.',
    'docs.wiki': 'Wiki',
    'docs.wikiNote': 'Install, models, workspace, skills, tools, MCP, sub-agents and FAQ',
    'docs.arch': 'Architecture',
    'docs.archNote': 'Layers, the request pipeline inside one turn, and sub-agent orchestration',
    'docs.readmeNote': 'Differentiators, configuration, project layout and security boundaries',
    'docs.releases': 'Release notes',
    'docs.releasesNote': 'Installers and changes for every version',

    'quote.body': 'The model handles cognition and planning; local tools handle execution and verification; the app owns boundaries and records.',
    'quote.note': 'No default persona, no hidden rewriting: your system message is the base, and only tool and skill instructions are appended.',

    'footer.license': 'MIT license',
    'footer.store': 'Microsoft Store',
    'footer.releases': 'Releases',
    'footer.licenseLink': 'License',
    'footer.privacy': 'Privacy policy',
    'footer.issues': 'Issues',
    'footer.note': 'This page loads no third-party scripts or trackers.',

    'os.win': 'Download for Windows',
    'os.mac': 'Download for macOS',
    'os.linux': 'Download for Linux',
    'detect.windows': 'Detected your system: Windows',
    'detect.mac': 'Detected your system: macOS (Apple Silicon or Intel)',
    'detect.linux': 'Detected your system: Linux',
    'copy.done': 'Copied',
    'copy.label': 'Copy',
    'theme.label': 'Toggle theme',
    'lang.label': '切换到中文'
  };

  var ZH_OS = {
    'os.win': '下载 Windows 版',
    'os.mac': '下载 macOS 版',
    'os.linux': '下载 Linux 版',
    'detect.windows': '已识别你的系统：Windows',
    'detect.mac': '已识别你的系统：macOS（Apple Silicon / Intel）',
    'detect.linux': '已识别你的系统：Linux',
    'copy.done': '已复制',
    'copy.label': '复制',
    'theme.label': '切换主题',
    'lang.label': 'Switch to English'
  };

  var root = document.documentElement;
  var nodes = Array.prototype.slice.call(document.querySelectorAll('[data-i18n],[data-i18n-html]'));
  var zh = {};
  nodes.forEach(function (el) {
    var isHtml = el.hasAttribute('data-i18n-html');
    var key = el.getAttribute(isHtml ? 'data-i18n-html' : 'data-i18n');
    if (!(key in zh)) zh[key] = isHtml ? el.innerHTML : el.textContent;
  });
  Object.keys(ZH_OS).forEach(function (key) { zh[key] = ZH_OS[key]; });

  var metaTitle = document.title;
  var metaDesc = document.querySelector('meta[name="description"]');
  var metaDescText = metaDesc ? metaDesc.getAttribute('content') : '';

  var lang = 'zh';
  var os = 'windows';

  var ASSETS = {
    windows: {
      href: 'https://github.com/xlbbb-cn/ai-chat/releases/latest/download/Rabbit-Chat-Studio-Windows-x64-setup.exe',
      card: 'windows'
    },
    mac: {
      href: 'https://github.com/xlbbb-cn/ai-chat/releases/latest/download/Rabbit-Chat-Studio-macOS-Apple-Silicon.dmg',
      card: 'mac-arm'
    },
    linux: {
      href: 'https://github.com/xlbbb-cn/ai-chat/releases/latest/download/Rabbit-Chat-Studio-Linux-x86_64.AppImage',
      card: 'linux'
    }
  };

  function t(key) {
    if (lang === 'en' && key in EN) return EN[key];
    return key in zh ? zh[key] : key;
  }

  function applyLanguage(next) {
    lang = next;
    root.setAttribute('lang', next === 'en' ? 'en' : 'zh-CN');
    nodes.forEach(function (el) {
      var isHtml = el.hasAttribute('data-i18n-html');
      var key = el.getAttribute(isHtml ? 'data-i18n-html' : 'data-i18n');
      if (isHtml) {
        var html = next === 'en' && key in EN ? EN[key] : zh[key];
        if (html) el.innerHTML = html;
      } else {
        var value = t(key);
        if (value) el.textContent = value;
      }
    });

    document.title = next === 'en'
      ? 'Rabbit Chat Studio — a desktop agent that knows its workspace'
      : metaTitle;

    if (metaDesc) {
      metaDesc.setAttribute('content', next === 'en'
        ? 'Turn any OpenAI-compatible model into a desktop agent with a workspace. Skills sandbox, self-evolving memory, sub-agent orchestration, MCP and local request audit. Windows, macOS and Linux; MIT licensed; no account required.'
        : metaDescText);
    }

    var langBtn = document.getElementById('lang-toggle');
    if (langBtn) {
      langBtn.textContent = next === 'en' ? '中文' : 'EN';
      langBtn.setAttribute('aria-label', t('lang.label'));
    }

    document.querySelectorAll('.copy').forEach(function (btn) {
      if (!btn.dataset.busy) btn.textContent = t('copy.label');
    });

    var themeBtn = document.getElementById('theme-toggle');
    if (themeBtn) themeBtn.setAttribute('aria-label', t('theme.label'));

    paintDownload();

    try { localStorage.setItem('rabbit-site-lang', next); } catch (e) {}
  }

  /* ── OS detection + download CTA ─────────────────────────────────── */

  function detectOS() {
    var ua = navigator.userAgent || '';
    var plat = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || '';
    var s = (plat + ' ' + ua).toLowerCase();
    if (s.indexOf('win') >= 0) return 'windows';
    if (s.indexOf('mac') >= 0 || s.indexOf('iphone') >= 0 || s.indexOf('ipad') >= 0) return 'mac';
    if (s.indexOf('linux') >= 0 || s.indexOf('x11') >= 0) return 'linux';
    return 'windows';
  }

  function paintDownload() {
    var asset = ASSETS[os];
    var hero = document.getElementById('hero-download');
    if (hero) {
      hero.href = asset.href;
      hero.textContent = t('os.' + (os === 'windows' ? 'win' : os));
    }

    document.querySelectorAll('.dl-card').forEach(function (card) {
      card.classList.toggle('is-current', card.getAttribute('data-os') === asset.card);
    });

    var detect = document.getElementById('detect');
    if (detect && os) {
      detect.hidden = false;
      detect.textContent = '';
      var dot = document.createElement('span');
      dot.className = 'dot';
      detect.appendChild(dot);
      detect.appendChild(document.createTextNode(t('detect.' + os)));
    }
  }

  /* ── Theme ───────────────────────────────────────────────────────── */

  var mq = window.matchMedia('(prefers-color-scheme: dark)');

  function resolvedTheme() {
    var explicit = root.getAttribute('data-theme');
    if (explicit === 'light' || explicit === 'dark') return explicit;
    return mq.matches ? 'dark' : 'light';
  }

  function toggleTheme() {
    var next = resolvedTheme() === 'dark' ? 'light' : 'dark';
    root.setAttribute('data-theme', next);
    try { localStorage.setItem('rabbit-site-theme', next); } catch (e) {}
  }

  /* ── Copy buttons ────────────────────────────────────────────────── */

  function copyText(text) {
    if (navigator.clipboard && window.isSecureContext) {
      return navigator.clipboard.writeText(text);
    }
    return new Promise(function (resolve, reject) {
      var ta = document.createElement('textarea');
      ta.value = text;
      ta.setAttribute('readonly', '');
      ta.style.position = 'fixed';
      ta.style.opacity = '0';
      document.body.appendChild(ta);
      ta.select();
      var ok = false;
      try { ok = document.execCommand('copy'); } catch (e) {}
      document.body.removeChild(ta);
      ok ? resolve() : reject();
    });
  }

  function flash(btn) {
    btn.dataset.busy = '1';
    btn.textContent = t('copy.done');
    btn.classList.add('is-done');
    window.setTimeout(function () {
      delete btn.dataset.busy;
      btn.textContent = t('copy.label');
      btn.classList.remove('is-done');
    }, 1600);
  }

  /* ── Wire up ─────────────────────────────────────────────────────── */

  os = detectOS();

  var savedLang = null;
  try { savedLang = localStorage.getItem('rabbit-site-lang'); } catch (e) {}
  var param = new URLSearchParams(window.location.search).get('lang');
  applyLanguage(param === 'en' || param === 'zh' ? param : savedLang === 'en' ? 'en' : 'zh');

  var themeBtn = document.getElementById('theme-toggle');
  if (themeBtn) themeBtn.addEventListener('click', toggleTheme);

  var langBtn = document.getElementById('lang-toggle');
  if (langBtn) {
    langBtn.addEventListener('click', function () {
      var next = lang === 'en' ? 'zh' : 'en';
      applyLanguage(next);
      try {
        var url = new URL(window.location.href);
        if (next === 'en') url.searchParams.set('lang', 'en');
        else url.searchParams.delete('lang');
        window.history.replaceState(null, '', url);
      } catch (e) {}
    });
  }

  document.querySelectorAll('.copy').forEach(function (btn) {
    btn.addEventListener('click', function () {
      var text = btn.getAttribute('data-copy') || '';
      copyText(text).then(function () { flash(btn); }, function () { flash(btn); });
    });
  });

  mq.addEventListener('change', function () {
    if (!root.getAttribute('data-theme')) paintDownload();
  });
})();
