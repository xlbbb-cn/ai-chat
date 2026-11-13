/* Rabbit Chat Studio — landing page behaviour.
   Theme, language, OS detection, copy buttons, FAQ and scroll spy.
   No dependencies, no third-party requests. */
(function () {
  'use strict';

  var EN = {
    'a11y.skip': 'Skip to content',
    'nav.features': 'Features',
    'nav.compare': 'Compare',
    'nav.boundaries': 'Boundaries',
    'nav.start': 'Get started',
    'nav.download': 'Download',
    'nav.docs': 'Docs',
    'nav.faq': 'FAQ',
    'nav.label': 'Main navigation',

    'hero.eyebrow': 'Tauri 2 · MIT · local-first',
    'hero.title': 'Put your AI<br><span class="accent">inside your workspace.</span>',
    'hero.sub': 'Any OpenAI-compatible model becomes a workspace-aware desktop agent.',
    'hero.copy': 'A skills sandbox, self-evolving memory, sub-agent orchestration and MCP, ready on first launch. Config, history and every request stay on this machine — <strong>no sign-up, no hidden prompt rewriting.</strong>',
    'hero.explore': 'Explore capabilities',
    'hero.repo': 'View on GitHub ↗',
    'hero.fine': 'Windows installer or Microsoft Store · macOS (Apple Silicon / Intel) · Linux AppImage. Point it at an API key or a local model and you are done.',
    'hero.visualLabel': 'Rabbit Chat Studio interface preview',
    'hero.trustLabel': 'Product highlights',
    'hero.tag': 'Your workspace is the agent\u2019s reach',
    'hero.mascotAlt': 'Rabbit Chat Studio bunny mascot',
    'hero.caption': 'One turn with tool calls, skills and a background task',

    'mock.user': 'Run the tests, fix only the failing one. Touch nothing else.',
    'mock.model': 'deepseek-v4 · 12 tools · evolution memory loaded',
    'mock.reply': 'Reading the workspace AGENTS.md first, then running the tests.',
    'mock.state1': 'done · 4.2s',
    'mock.state2': 'running in background',
    'mock.placeholder': 'Message, press Enter to send…',
    'mock.send': 'Send',

    'trust.local': 'Local-first',
    'trust.account': 'No account',
    'trust.api': 'OpenAI-compatible',
    'trust.mit': 'Open source · MIT',

    'cap.kicker': 'Chat is only the start',
    'cap.title': 'One client for the whole workflow.',
    'cap.lede': 'Give the agent a workspace, tools that actually run, and an audit trail you can read.',
    'cap.skills.title': 'Skills sandbox',
    'cap.skills.body': 'Each workspace has exactly one skills root, <code>.data/skills</code>, and every skill is locked to its own SKILL.md. Enablement is stored per workspace too.',
    'cap.evo.title': 'Self-evolution',
    'cap.evo.body': 'The assistant installs, edits and deletes its own skills, and writes verified facts into evolution memory. Every change leaves a <code>.bak.n</code> backup.',
    'cap.agents.title': 'Sub-agent orchestration',
    'cap.agents.body': 'Three runtimes — llm, cli and cloud — with file-based definitions at <code>agents/&lt;category&gt;/&lt;slug&gt;.md</code>. Model, tool allowlist and MCP are configured per agent.',
    'cap.tools.title': 'Built-in tools',
    'cap.tools.body': 'Four memory scopes, todos, one-shot timers, file operations and patches. <code>run_cmd</code> runs a PATH program; <code>run_shell</code> scripts pass the risk engine first.',
    'cap.mcp.title': 'MCP',
    'cap.mcp.body': 'stdio, SSE, HTTP and Streamable HTTP, with a connectivity test. Each sub-agent task keeps its own server process and exits when the task ends.',
    'cap.audit.title': 'Local audit & background runs',
    'cap.audit.body': 'Request body, tokens, latency, status and error go into local SQLite. Slow tools, MCP calls and sub-agents can run in the background while the model keeps going.',

    'show.kicker': 'Built for the desktop',
    'show.title': 'Everything you need, within reach.',
    'show.lede': 'A focused chat workspace with quick access to Agents, Skills, MCP, Tools, History and Settings — without locking you into a bundled model.',
    'show.check1': 'Connect any OpenAI-compatible API.',
    'show.check2': 'Use your own API key and preferred model.',
    'show.check3': 'Extend the workflow with Agents, Skills, MCP and Tools.',
    'show.check4': 'Reference files and manage conversations in one window.',
    'show.badge': 'Real UI',
    'show.shotAlt': 'Rabbit Chat Studio desktop interface showing chat, Agents, Skills, MCP, Tools, History and Settings',

    'cmp.kicker': 'More than a chat box',
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
    'mark.yes': 'native',
    'mark.part': 'partial or adjacent',
    'mark.no': 'not offered',

    'sec.kicker': 'Capability and boundaries',
    'sec.title': 'Capability needs a written boundary.',
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

    'start.kicker': 'Three steps',
    'start.title': 'Install, connect, pick a workspace.',
    'start.lede': 'Rabbit Chat Studio is the client. You bring the model connection.',
    'start.s1.title': 'Install the app',
    'start.s1.body': 'Download the installer for your platform. Windows ships an NSIS installer; macOS gets separate Apple Silicon and Intel images.',
    'start.s2.title': 'Connect a model',
    'start.s2.body': 'An OpenAI-compatible base URL plus an API key, or a local gateway. No account, no forced sign-in.',
    'start.s3.title': 'Pick a workspace',
    'start.s3.body': 'The workspace is the agent\u2019s reach: skills, memory, todos and the database all live in its .data directory.',
    'start.privacy.title': 'Privacy is part of the architecture',
    'start.privacy.body': 'No Rabbit account, no telemetry SDK, no ads. Prompts and any files you send to a model are processed by the API provider you configure; check that provider\u2019s data and privacy policies. <a href="./privacy-policy.html">Read the privacy policy</a>.',

    'dl.kicker': 'Download',
    'dl.title': 'Pick a platform. Start your workflow.',
    'dl.lede': 'Windows installer or Microsoft Store, macOS (Apple Silicon / Intel), Linux AppImage. The releases page always has the latest version.',
    'dl.versionLabel': 'Latest release: ',
    'dl.current': 'your system',
    'dl.winNote': 'x64 · NSIS installer',
    'dl.storeNote': 'Windows 10 2004+ · installed and updated by the Store',
    'dl.macArmNote': 'M-series chips',
    'dl.macIntelNote': 'Intel chips',
    'dl.linuxNote': 'x86_64 · AppImage',
    'dl.pkgTitle': 'Open the Store page',
    'dl.pkgNote': 'Paste this into the Run box (Win+R) or your address bar to open the Store page. winget, Homebrew and Scoop manifests are not available yet.',
    'dl.devTitle': 'Build from source',
    'dl.devReq': 'Requires Node.js 20.19+ (or 22.12+) and a stable Rust toolchain; Windows should use the MSVC toolchain.',

    'docs.kicker': 'Documentation',
    'docs.title': 'From install to sub-agents, it is written down.',
    'docs.lede': 'The Wiki carries the full bilingual guide; the architecture page, README and release notes live in the repository.',
    'docs.wikiNote': 'Install, models, workspace, skills, tools, MCP, sub-agents and FAQ',
    'docs.arch': 'Architecture',
    'docs.archNote': 'Layers, the request pipeline inside one turn, and sub-agent orchestration',
    'docs.readmeNote': 'Differentiators, configuration, project layout and security boundaries',
    'docs.releases': 'Release notes',
    'docs.releasesNote': 'Installers and changes for every version',

    'quote.body': 'The model handles cognition and planning; local tools handle execution and verification; the app owns boundaries and records.',
    'quote.note': 'No default persona, no hidden rewriting: your system message is the base, and only tool and skill instructions are appended.',

    'faq.kicker': 'FAQ',
    'faq.title': 'What you might want to know first.',
    'faq.q1': 'Does Rabbit Chat Studio include an AI model?',
    'faq.a1': 'No. Rabbit Chat Studio is a desktop client, not a model. You connect it to a model service with your own API endpoint and API key.',
    'faq.q2': 'Do I need to create an account?',
    'faq.a2': 'No Rabbit Chat Studio account is required. You only need the API details from the provider you choose.',
    'faq.q3': 'Which API providers can I use?',
    'faq.a3': 'Any service that speaks the OpenAI-style Chat Completions interface, or a local gateway. Compatibility varies by provider and by supported API features, so check the endpoint and model requirements first.',
    'faq.q4': 'Where do my data and requests go?',
    'faq.a4': 'Config, history, audit records and agent state live on this machine, in the app data directory and the workspace <code>.data</code> folder. To get a reply, prompts and attachments are sent to the API endpoint you configure; review that provider\u2019s privacy and retention policy before sending sensitive content.',
    'faq.q5': 'Is Rabbit Chat Studio free?',
    'faq.a5': 'The client is open source under the MIT license: use, modify and redistribute it freely. Model usage is billed separately by the provider you choose, at that provider\u2019s prices.',
    'faq.q6': 'Which platforms are supported?',
    'faq.a6': 'Windows 10 2004+ (NSIS installer or Microsoft Store), macOS (Apple Silicon and Intel) and Linux (x86_64 AppImage). winget, Homebrew and Scoop manifests are not available yet.',
    'faq.q7': 'Are Skills and self-evolution safe?',
    'faq.a7': 'Each workspace has a single skills root, file operations are confined to the workspace, and skill edits leave .bak backups. Self-evolution lets the assistant rewrite its own instructions: use it in a controlled workspace, keep it short, and review the backups afterwards. MCP arguments are forwarded as-is — verify least privilege before connecting an external service.',

    'band.label': 'Download',
    'band.title': 'Make AI work your way.',
    'band.copy': 'Rabbit Chat Studio — your desktop, your API, your workflow.',
    'band.releases': 'All releases ↗',

    'footer.license': 'MIT license',
    'footer.rights': 'All rights reserved.',
    'footer.store': 'Microsoft Store',
    'footer.releases': 'Releases',
    'footer.licenseLink': 'License',
    'footer.privacy': 'Privacy policy',
    'footer.issues': 'Issues',
    'footer.label': 'Footer',
    'footer.note': 'This page loads no third-party scripts or trackers.'
  };

  /* Strings that are painted by script and therefore are not in the markup. */
  var ZH_DYNAMIC = {
    'os.win': '下载 Windows 版',
    'os.mac': '下载 macOS 版',
    'os.linux': '下载 Linux 版',
    'detect.windows': '已识别你的系统：Windows',
    'detect.mac': '已识别你的系统：macOS（Apple Silicon / Intel）',
    'detect.linux': '已识别你的系统：Linux',
    'copy.label': '复制',
    'copy.done': '已复制',
    'theme.label': '切换主题',
    'lang.label': 'Switch to English',
    'menu.open': '打开导航菜单',
    'menu.close': '关闭导航菜单'
  };

  var EN_DYNAMIC = {
    'os.win': 'Download for Windows',
    'os.mac': 'Download for macOS',
    'os.linux': 'Download for Linux',
    'detect.windows': 'Detected your system: Windows',
    'detect.mac': 'Detected your system: macOS (Apple Silicon or Intel)',
    'detect.linux': 'Detected your system: Linux',
    'copy.label': 'Copy',
    'copy.done': 'Copied',
    'theme.label': 'Toggle theme',
    'lang.label': '切换到中文',
    'menu.open': 'Open navigation menu',
    'menu.close': 'Close navigation menu'
  };

  var root = document.documentElement;
  var nodes = Array.prototype.slice.call(document.querySelectorAll('[data-i18n],[data-i18n-html],[data-i18n-aria],[data-i18n-alt]'));
  var zhText = {};
  var zhHtml = {};
  var zhAria = {};
  var zhAlt = {};

  nodes.forEach(function (el) {
    var key;
    if ((key = el.getAttribute('data-i18n'))) zhText[key] = el.textContent;
    if ((key = el.getAttribute('data-i18n-html'))) zhHtml[key] = el.innerHTML;
    if ((key = el.getAttribute('data-i18n-aria'))) zhAria[key] = el.getAttribute('aria-label');
    if ((key = el.getAttribute('data-i18n-alt'))) zhAlt[key] = el.getAttribute('alt');
  });

  var metaTitle = document.title;
  var metaDesc = document.querySelector('meta[name="description"]');
  var metaDescText = metaDesc ? metaDesc.getAttribute('content') : '';
  var metaTitleEn = 'Rabbit Chat Studio — a workspace-aware desktop AI agent';
  var metaDescEn = 'Turn any OpenAI-compatible model into a workspace-aware desktop agent. Skills sandbox, self-evolving memory, sub-agent orchestration, MCP and local request audit. Windows, macOS and Linux; MIT licensed; no account required.';

  var lang = 'zh';
  var os = 'windows';
  var menuOpen = false;

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
    if (key in zhText) return zhText[key];
    if (key in zhHtml) return zhHtml[key];
    if (key in zhAria) return zhAria[key];
    if (key in zhAlt) return zhAlt[key];
    if (lang === 'en' && key in EN_DYNAMIC) return EN_DYNAMIC[key];
    if (key in ZH_DYNAMIC) return ZH_DYNAMIC[key];
    return key;
  }

  function applyLanguage(next) {
    lang = next;
    root.setAttribute('lang', next === 'en' ? 'en' : 'zh-CN');

    nodes.forEach(function (el) {
      var key;
      if ((key = el.getAttribute('data-i18n'))) {
        el.textContent = t(key);
      } else if ((key = el.getAttribute('data-i18n-html'))) {
        var html = next === 'en' && key in EN ? EN[key] : zhHtml[key];
        if (html) el.innerHTML = html;
      } else if ((key = el.getAttribute('data-i18n-aria'))) {
        var aria = next === 'en' && key in EN ? EN[key] : zhAria[key];
        if (aria) el.setAttribute('aria-label', aria);
      } else if ((key = el.getAttribute('data-i18n-alt'))) {
        var alt = next === 'en' && key in EN ? EN[key] : zhAlt[key];
        if (alt) el.setAttribute('alt', alt);
      }
    });

    document.title = next === 'en' ? metaTitleEn : metaTitle;
    if (metaDesc) metaDesc.setAttribute('content', next === 'en' ? metaDescEn : metaDescText);

    var ogTitle = document.querySelector('meta[property="og:title"]');
    var ogDesc = document.querySelector('meta[property="og:description"]');
    var twTitle = document.querySelector('meta[name="twitter:title"]');
    var twDesc = document.querySelector('meta[name="twitter:description"]');
    if (ogTitle) ogTitle.setAttribute('content', next === 'en' ? metaTitleEn : metaTitle);
    if (twTitle) twTitle.setAttribute('content', next === 'en' ? metaTitleEn : metaTitle);
    if (ogDesc) ogDesc.setAttribute('content', next === 'en' ? metaDescEn : metaDescText);
    if (twDesc) twDesc.setAttribute('content', next === 'en' ? metaDescEn : metaDescText);

    var langBtn = document.getElementById('lang-toggle');
    if (langBtn) {
      langBtn.textContent = next === 'en' ? '中文' : 'EN';
      langBtn.setAttribute('aria-label', t('lang.label'));
    }

    var themeBtn = document.getElementById('theme-toggle');
    if (themeBtn) {
      themeBtn.setAttribute('aria-label', t('theme.label'));
      themeBtn.setAttribute('title', t('theme.label'));
    }

    var menuBtn = document.getElementById('menuToggle');
    if (menuBtn) menuBtn.setAttribute('aria-label', t(menuOpen ? 'menu.close' : 'menu.open'));

    document.querySelectorAll('.copy').forEach(function (btn) {
      if (!btn.dataset.busy) btn.textContent = t('copy.label');
    });

    paintDownload();
    updateActive();
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

    document.querySelectorAll('[data-download]').forEach(function (link) {
      link.href = asset.href;
    });
    document.querySelectorAll('[data-download-label]').forEach(function (label) {
      label.textContent = t('os.' + (os === 'windows' ? 'win' : os));
    });

    document.querySelectorAll('.dl-card').forEach(function (card) {
      card.classList.toggle('is-current', card.getAttribute('data-os') === asset.card);
    });

    var detect = document.getElementById('detect');
    if (detect) {
      detect.textContent = '';
      detect.hidden = false;
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

  /* ── Mobile navigation + scroll spy ──────────────────────────────── */

  var menuToggle = document.getElementById('menuToggle');
  var navLinks = document.getElementById('navLinks');

  function setMenu(open) {
    if (!menuToggle || !navLinks) return;
    menuOpen = open;
    navLinks.classList.toggle('show', open);
    menuToggle.setAttribute('aria-expanded', String(open));
    menuToggle.setAttribute('aria-label', t(open ? 'menu.close' : 'menu.open'));
    menuToggle.textContent = open ? '✕' : '☰';
  }

  if (menuToggle && navLinks) {
    menuToggle.addEventListener('click', function () {
      setMenu(!menuOpen);
    });
    navLinks.querySelectorAll('a').forEach(function (link) {
      link.addEventListener('click', function () { setMenu(false); });
    });
    document.addEventListener('click', function (event) {
      if (!menuOpen) return;
      if (navLinks.contains(event.target) || menuToggle.contains(event.target)) return;
      setMenu(false);
    });
    document.addEventListener('keydown', function (event) {
      if (event.key === 'Escape' && menuOpen) setMenu(false);
    });
  }

  var navAnchors = Array.prototype.slice.call(document.querySelectorAll('.nav-links a[href^="#"]'));
  var navSections = navAnchors.map(function (a) {
    return document.getElementById(a.getAttribute('href').slice(1));
  }).filter(Boolean);
  var scrollTick = false;

  function updateActive() {
    var y = window.scrollY + 130;
    var active = null;
    navSections.forEach(function (section) {
      if (section.offsetTop <= y) active = section.id;
    });
    navAnchors.forEach(function (a) {
      a.classList.toggle('is-active', a.getAttribute('href') === '#' + active);
    });
  }

  function requestActive() {
    if (scrollTick) return;
    scrollTick = true;
    window.requestAnimationFrame(function () {
      updateActive();
      scrollTick = false;
    });
  }

  window.addEventListener('scroll', requestActive, { passive: true });
  window.addEventListener('resize', requestActive);

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

  document.querySelectorAll('.faq-question').forEach(function (button) {
    button.addEventListener('click', function () {
      var item = button.closest('.faq-item');
      var open = item.classList.toggle('open');
      button.setAttribute('aria-expanded', String(open));
    });
  });

  var year = document.getElementById('year');
  if (year) year.textContent = new Date().getFullYear();

  mq.addEventListener('change', function () {
    if (!root.getAttribute('data-theme')) updateActive();
  });

  updateActive();
})();
