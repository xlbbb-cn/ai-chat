const fs = require('fs');

let app = fs.readFileSync('src/App.tsx', 'utf8');

app = app.replace('import { chatCompletion } from "./api";', 
'import { chatCompletion, searchDuckduckgo, saveHistory, loadHistory, HistoryRecord } from "./api";');

app = app.replace('const history = [...messages, userMsg].map((m) => ({ role: m.role, content: m.content }));',
`const history = [...messages, userMsg].map((m) => ({ role: m.role, content: m.content }));
    
    saveHistory(sessionId, "user", text);

    if (webSearch) {
      try {
        const searchCtx = await searchDuckduckgo(text);
        if (history.length > 0) {
            history[history.length - 1].content = "Web Context:\n" + searchCtx + "\n\nUser Question:\n" + text;
        }
      } catch (e) {
        console.error(e);
      }
    }`);

app = app.replace('onDone() {',
`onDone() {
        setMessages((prev) => {
           let finalMsg = prev.find(m => m.id === assistantId);
           if (finalMsg) saveHistory(sessionId, "assistant", finalMsg.content);
           return prev;
        });`);

app = app.replace('    setMessages([]);',
`    setMessages([]);
    setSessionId(crypto.randomUUID());`);

app = app.replace('title="Settings"', 'title="Settings"');

const historyBtn = `<button
              className={\`toolbar-btn \${sidebar === "history" ? "active" : ""}\`}
              onClick={() => toggleSidebar("history")}
              title="History"
            >
              🕒 History
            </button>`;

app = app.replace('✦ Skills', `✦ Skills\n            </button>\n            ${historyBtn}`);

const webBtn = `<label className="toolbar-btn" style={{display: 'flex', alignItems: 'center', gap: '5px'}}>
              <input type="checkbox" checked={webSearch} onChange={e => setWebSearch(e.target.checked)} />
              Web
            </label>`;
            
app = app.replace('<button className="toolbar-btn" onClick={clearChat} title="New chat">',
`${webBtn}\n            <button className="toolbar-btn" onClick={clearChat} title="New chat">`);


const historyPanel = `
          {sidebar === "history" && (
            <div className="settings-panel">
               <div className="panel-header">
                 <h2>History</h2>
                 <button className="icon-btn" onClick={() => setSidebar(null)}>✕</button>
               </div>
               <div className="panel-content">
                 <button onClick={async () => {
                    const recs = await loadHistory();
                    alert(recs.length + ' msgs saved in db. check console.');
                    console.log(recs);
                 }}>Load History</button>
               </div>
            </div>
          )}
`;
app = app.replace('</aside>', `${historyPanel}\n        </aside>`);

fs.writeFileSync('src/App.tsx', app);
