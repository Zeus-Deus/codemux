// Concept 02: all operations stay in this page's memory. No live IPC,
// notification permission requests, service worker registration, or uploads.
Object.assign(paths, {
  home: 'M3 10 12 3l9 7v11h-6v-7H9v7H3Z',
  share: 'M12 15V2 M8 6l4-4 4 4 M5 10H3v12h18V10h-2',
  phone: 'M6 2h12v20H6Z M10 18h4',
  keyboard: 'M2 5h20v14H2Z M6 9h.01 M10 9h.01 M14 9h.01 M18 9h.01 M6 13h.01 M10 13h8',
});
const baseRender = render;
const baseMore = renderMore;
const baseTerminal = renderTerminal;
const homeNames = workspaces.map(w => w[0]);
const memory = new Map();
const selectedSessions = new Map();
Object.assign(state, {
  view: 'home', session: 'Builder', connected: true, homeFilter: 'All',
  homeSearch: '', installDismissed: false, installedPreview: false,
  notifications: 'off', platform: /Android/i.test(navigator.userAgent) ? 'android' : 'ios',
  queued: [], reviewed: new Set(), reviewComments: {}, reviewIndex: 0, scroll: 0,
});
const scenarios = {
  'Mobile remote control': {status:'review',label:'Ready for review',project:'codemux',branch:'feat/mobile-remote',meta:'3 files · +128 −36',prompt:'Make the workspace feel at home on a phone. Keep all the tools within reach.',answer:'The mobile shell is ready for review. Chat, files, and terminal each have their own space, with your desktop still doing the work.'},
  'Fix reconnect behavior': {status:'attention',label:'Needs you',project:'codemux',branch:'fix/reconnect',meta:'Waiting for your answer',prompt:'Keep my draft when the phone reconnects.',answer:'Draft recovery is in place. One choice before I finish: should a draft send automatically when the connection comes back?'},
  'Refresh the homepage': {status:'working',label:'Working',project:'fieldnotes',branch:'feat/homepage',meta:'Building the page · 2m',prompt:'Give the homepage a calmer, more considered feel.',answer:'I’m refining the page spacing and checking the smaller breakpoints. You can queue a follow-up while I finish.'},
};
function scenario() { return scenarios[state.workspace] || {status:'new',label:'New workspace',project:'codemux',branch:'new-workspace',meta:'Ready to start',prompt:'',answer:'Your workspace is ready. What would you like to build?'}; }
function key() { return `${state.workspace}/${state.session}`; }
function remember() {
  const transcript = document.querySelector('.transcript');
  state.scroll=transcript?.scrollTop ?? state.scroll;
  memory.set(key(), {draft:state.draft,messages:[...state.messages],queued:[...state.queued],attachment:state.attachment,scroll:transcript?.scrollTop ?? state.scroll,approved:state.approved,running:state.running,model:state.model});
}
function restore() {
  const saved = memory.get(key());
  Object.assign(state, saved || {draft:'',messages:[],queued:[],attachment:'',scroll:0,approved:false,running:scenario().status==='working',model:state.session==='Reviewer'?'Claude · Sonnet':'Codex · GPT-5.4'});
}
function openWorkspace(name) {
  remember(); state.workspace=name; state.session=selectedSessions.get(name)||'Builder';
  restore(); state.view='chat'; if(sheet.open)closeSheet(); render();
}
function go(view) { remember(); state.view=view; if(sheet.open)closeSheet(); render(); }
function actionButton(label,action,ico='chevron',value='') { return `<button class="list-row" data-action="${action}" data-value="${esc(value)}"><span class="grow"><strong>${label}</strong></span>${icon(ico)}</button>`; }
function statusLabel(name) {
  const item=scenarios[name];
  if(name===state.workspace && state.approved) return 'Answer sent';
  return item?.label || 'New workspace';
}
function homeRows() {
  const names=workspaces.map(w=>w[0]).filter(n=>`${n} ${scenarios[n]?.project||'codemux'}`.toLowerCase().includes(state.homeSearch.toLowerCase()));
  const groups=[['Needs you','attention'],['In progress','working'],['Ready for review','review'],['New workspaces','new']];
  return groups.map(([label,status])=>{
    const subset=names.filter(n=>(scenarios[n]?.status||'new')===status && (state.homeFilter==='All'||(state.homeFilter==='Needs you'&&status==='attention')||(state.homeFilter==='Working'&&status==='working')));
    if(!subset.length)return '';
    return `<section class="workspace-group"><div class="eyebrow">${label}<span>${subset.length}</span></div>${subset.map(n=>`<button class="workspace-row" data-action="workspace" data-value="${esc(n)}"><span class="workspace-glyph ${status}">${icon(status==='attention'?'bell':status==='working'?'clock':'branch')}</span><span class="grow"><strong>${esc(n)}</strong><small>${esc(scenarios[n]?.project||'codemux')} <span>·</span> ${esc(scenarios[n]?.meta||'Ready to start')}</small><span class="status-text ${status}"><span class="status-dot"></span>${statusLabel(n)}</span></span>${icon('chevron')}</button>`).join('')}</section>`;
  }).join('') || '<p class="hint">No matching workspaces.</p>';
}
function renderHome() {
  main.className='';
  main.innerHTML=`<div class="page home-page"><div class="home-heading"><div><div class="eyebrow">Your desktop, with you</div><h1>Workspaces</h1></div><button class="icon-button" data-action="notifications" aria-label="Notification preferences">${icon('bell')}</button></div><div class="filter-row" role="group" aria-label="Workspace filters">${['All','Needs you','Working'].map(f=>`<button class="filter ${state.homeFilter===f?'selected':''}" aria-pressed="${state.homeFilter===f}" data-action="home-filter" data-value="${f}">${f}${f==='Needs you'?`<span class="filter-count">${Object.values(scenarios).filter(s=>s.status==='attention').length}</span>`:''}</button>`).join('')}</div><div id="home-results">${homeRows()}</div>${!state.installDismissed&&!state.installedPreview?`<div class="install-card"><div class="row"><span class="install-glyph">${icon('phone')}</span><div class="grow"><strong>Make room on your Home Screen.</strong><p>Open Codemux like an app. Get notified when your agents need you.</p></div><button class="icon-button" data-action="dismiss-install" aria-label="Dismiss installation suggestion">${icon('close')}</button></div><button class="install-link" data-action="install">See how to install ${icon('chevron')}</button></div>`:''}${state.installedPreview?`<button class="change-card" data-action="notifications">${icon('bell')}<span class="grow"><strong>${state.notifications==='enabled'?'Notifications enabled · demo':'Know when your work needs you'}</strong><small>${state.notifications==='enabled'?'Manage what reaches your phone':'Choose your notification preferences'}</small></span>${icon('chevron')}</button>`:''}<p class="demo-footnote">Interactive concept · no live workspaces</p></div>`;
  document.querySelector('#nav').innerHTML=`<button class="secondary" data-action="command-search" aria-label="Search workspaces and actions">${icon('search')}</button><button class="primary grow" data-action="new">${icon('plus')} New workspace</button>`;
  document.querySelector('#nav').classList.add('home-nav');
}
render = function() {
  if(state.view==='home')renderHome();
  else if(state.view==='review-file')renderReviewFile();
  else baseRender();
  const home=state.view==='home';
  document.querySelector('#nav').classList.toggle('home-nav',home);
  document.querySelector('#nav').hidden=state.view==='review-file';
  const picker=document.querySelector('.workspace-picker');
  picker.innerHTML=home?`<span class="project-mark">C</span><span><strong id="workspace-title">Codemux</strong><small>Development desktop</small></span>`:`<span class="header-back">${icon('back')}</span><span><strong id="workspace-title">${esc(state.workspace)}</strong><small>${esc(scenario().project)} ${icon('down')}</small></span>`;
  picker.dataset.action=home?'connection':'home';
  picker.setAttribute('aria-label',home?'Choose connected desktop':'Back to workspaces');
  document.querySelector('.context').innerHTML=`<span class="branch">${icon(home?'wifi':'branch')}<span id="branch-name">${home?(state.connected?'Desktop connected':'Reconnecting…'):esc(scenario().branch)}</span></span><button class="demo-badge" data-action="about">Concept / 02</button>`;
  document.querySelector('.connection').innerHTML=`<span class="live-dot ${state.connected?'':'offline'}"></span>${icon('wifi')}`;
  document.querySelector('.connection').setAttribute('aria-label',state.connected?'Connection details':'Reconnecting to desktop');
  if(!state.connected && state.view!=='home') {
    const banner=document.createElement('div');banner.className='offline-strip';banner.innerHTML=`${icon('wifi')}<span>Reconnecting · your draft is safe</span><button data-action="restore-connection">Retry</button>`;
    main.prepend(banner);
  }
  icons();
  const transcript=main.querySelector('.transcript'); if(transcript)transcript.scrollTop=state.scroll||0;
};
renderChat = function() {
  const s=scenario(), question=state.workspace===homeNames[1]&&!scenarios[homeNames[1]].answered;
  const reviewer=state.session==='Reviewer';
  let pending=question?`<div class="pending-strip attention"><span>${icon('bell')} Codex needs your input</span><button data-action="answer-question">Answer ${icon('chevron')}</button></div>`:
    state.queued.length?`<div class="pending-strip"><span>${icon('clock')} ${state.queued.length} follow-up queued</span><button data-action="queued">View ${icon('chevron')}</button></div>`:
    state.running?`<div class="pending-strip"><span>${icon('clock')} Working · 2 background tasks</span><button data-action="tasks">View ${icon('chevron')}</button></div>`:'';
  main.innerHTML=`<div class="session-bar"><button data-action="sessions">${icon('chat')} ${esc(state.session)} <span class="muted">${reviewer?'Claude':'Codex'}</span>${icon('down')}</button><button class="icon-button" data-action="command-search" aria-label="Search actions">${icon('search')}</button></div><section class="transcript" aria-label="Conversation"><div class="day">Today · ${reviewer?'Code review':'Current session'}</div>${s.prompt?`<div class="user-message">${esc(reviewer?'Review these changes and flag anything I should fix.':s.prompt)}</div>`:''}<div class="agent-label"><span class="agent-symbol">✳</span>${reviewer?'Claude':'Codex'}<small>Just now</small></div><button class="activity" data-action="activity">${icon('check')} Explored 6 files · worked for 42s <span class="end">${icon('chevron')}</span></button><p class="agent-message">${esc(reviewer?'The changes are scoped well. I’m checking navigation, keyboard behavior, and reconnect recovery. Open Changes to review the files with me.':s.answer)}</p>${state.approved?'<p class="answer-receipt">✓ Your answer was sent in this demo.</p>':''}${s.status==='review'?`<button class="change-card" data-action="view" data-value="changes">${icon('changes')}<span class="grow"><strong>Review 3 changed files</strong><small>2 checks passed · PR #42 +1</small></span><span class="counts positive">+128</span>${icon('chevron')}</button>`:''}${state.messages.map(m=>`<div class="user-message">${esc(m)}</div><p class="agent-message muted">Received in the demo conversation.</p>`).join('')}</section><form class="composer-wrap" id="chat-form">${pending}<div class="composer">${state.attachment?`<div class="attachment-chip">${esc(state.attachment)}</div>`:''}<div class="slash-hint" hidden></div><textarea aria-label="Message the agent" placeholder="${state.connected?'Ask, build, or steer…':'Keep writing. Send when reconnected.'}" rows="2">${esc(state.draft)}</textarea><div class="composer-tools"><button type="button" class="icon-button" data-action="attach" aria-label="Attach a file">${icon('plus')}</button><button type="button" class="model" data-action="model">${esc(state.model)}${icon('down')}</button><button type="button" class="icon-button dismiss-keyboard" data-action="dismiss-keyboard" aria-label="Dismiss keyboard">${icon('keyboard')}</button>${state.running?`<button type="button" class="icon-button" data-action="stop" aria-label="Stop agent">${icon('stop')}</button>`:''}<button class="primary send" type="submit" aria-label="${state.running?'Queue follow-up':'Send message'}" ${!state.connected?'disabled':''}>${icon(state.running?'plus':'arrow')}</button></div></div><div class="composer-note">${state.connected?(state.running?'Send a follow-up to the queue':'Demo session · changes stay in this page'):'Offline · draft kept on this page'}</div></form>`;
  const input=main.querySelector('textarea');
  input.addEventListener('input',e=>{state.draft=e.target.value;const hint=main.querySelector('.slash-hint');hint.hidden=!state.draft.startsWith('/goal');hint.textContent='/goal · Work toward a goal';});
  main.querySelector('form').addEventListener('submit',e=>{
    e.preventDefault();if(!state.connected||!state.draft.trim())return;
    if(state.running)state.queued.push(state.draft);else state.messages.push(state.draft);
    state.draft='';state.attachment='';state.scroll=999999;render();main.querySelector('textarea').focus();
  });
};
function sessionSheet() {
  openSheet('Agents & sessions',`<p class="hint">${esc(state.workspace)} · your selection stays on this device.</p><div class="list">${['Builder','Reviewer'].map(name=>`<button class="list-row" data-action="choose-session" data-value="${name}">${icon('chat')}<span class="grow"><strong>${name}</strong><small>${name==='Builder'?'Codex · implementation':'Claude · code review'}</small></span>${state.session===name?icon('check'):icon('chevron')}</button>`).join('')}${row('Shell 1','Interactive desktop terminal','terminal','view').replace('data-action="view"','data-action="view" data-value="terminal"')}${row('Browser','App preview · localhost:3000','globe','view-preview')}</div>`);
}
function installSheet() {
  const ios=state.platform==='ios';
  openSheet('Keep Codemux close.',`<div class="prototype-notice">Onboarding preview · intended for app.codemux.org</div><p class="install-intro">Your workspaces, a tap away.<br>Open from your Home Screen without the browser toolbar.</p><div class="segmented" role="group" aria-label="Installation platform"><button data-action="platform" data-value="ios" aria-pressed="${ios}">iPhone / iPad</button><button data-action="platform" data-value="android" aria-pressed="${!ios}">Android</button></div><ol class="install-steps">${(ios?[
    ['Open Codemux in Safari','Visit app.codemux.org and sign in.'],
    ['Tap Share, then Add to Home Screen','You may need to scroll through the Share menu.'],
    ['Add it, then open the Codemux icon','If offered, leave Open as Web App enabled.']
  ]:[['Open Codemux in Chrome','Visit app.codemux.org and sign in.'],['Choose Install app','Look in the browser menu for Install app or Add to Home screen. Wording varies by browser.'],['Confirm and open Codemux','Launch the new icon from your Home Screen.']]).map(([t,d],i)=>`<li><span>${i+1}</span><div><strong>${t}</strong><p>${d}</p></div></li>`).join('')}</ol><button class="primary full" data-action="preview-installed">Preview the installed experience ${icon('chevron')}</button><p class="hint">This PassPage does not install Codemux. These steps preview the future hosted app.</p>`);
}
function notificationsSheet() {
  if(!state.installedPreview){openSheet('Bring updates to your phone',`<div class="prototype-notice">Notification setup preview</div><div class="notification-illustration">${icon('bell')}</div><h3>A question answered sooner.<br>A finished task you won’t miss.</h3><p class="hint">On iPhone, add Codemux to your Home Screen and open it there before enabling notifications. Android support depends on your browser.</p><button class="primary full" data-action="install">See installation steps</button><button class="secondary full spaced" data-action="preview-installed">Preview an installed app</button>`);return;}
  const blocked=state.notifications==='blocked',enabled=state.notifications==='enabled';
  openSheet('Only the updates that matter',`<div class="prototype-notice">Installed-app preview · no real permission requested</div><div class="notification-sample"><div class="row between"><span class="eyebrow">CODEMUX</span><small>now</small></div><strong>Codex needs your input</strong><p>Fix reconnect behavior · a question is waiting.</p></div>${blocked?'<h3>Notifications are blocked</h3><p class="hint">You can keep using Codemux. To receive alerts, allow notifications in your device or browser settings, then check again.</p>':`<p class="hint">${enabled?'Enabled in this demo. Choose which updates should reach your phone.':'Get an alert when an agent needs a decision or your work is ready to review. You can change this later.'}</p>`}<div class="notification-options">${[['questions','Questions & approvals'],['finished','Work ready for review'],['failures','Run failures']].map(([id,label])=>`<label><span>${label}</span><input type="checkbox" data-notification="${id}" ${state['notify_'+id]===false?'':'checked'}></label>`).join('')}</div><button class="primary full" data-action="${enabled?'test-notification':blocked?'reset-permission':'enable-notifications'}">${enabled?'Show a sample notification':blocked?'Preview permission reset':'Enable notifications · demo'}</button><button class="text-action full" data-action="${enabled?'disable-notifications':'later'}">${enabled?'Turn off in demo':'Not now'}</button>${!enabled&&!blocked?'<button class="text-action full muted" data-action="block-notifications">Preview a blocked permission</button>':''}<p class="demo-footnote">No push service is connected to this prototype.</p>`);
}
function renderReviewFile() {
  main.className='review-screen';const i=state.reviewIndex,f=files[i];
  const snippets=[
    [['note','@@ -12,6 +12,9 @@'],['','  return ('],['remove','−   <div className="h-screen">'],['add','+   <div className="mobile-shell">'],['add','+     <WorkspaceHeader />'],['','      <WorkspaceContent />'],['add','+     <MobileNavigation />'],['','    </div>'],['','  );']],
    [['note','@@ -1,4 +1,8 @@'],['add','+ export const views = ['],['add','+   "chat", "changes", "files",'],['add','+   "terminal", "more"'],['add','+ ];'],['',''],['add','+ // Keep selection on this device.']],
    [['note','@@ -24,3 +24,7 @@'],['add','+ .mobile-shell {'],['add','+   min-height: 100dvh;'],['add','+   padding-bottom:'],['add','+     env(safe-area-inset-bottom);'],['add','+ }']]
  ];
  main.innerHTML=`<div class="review-top"><button class="back" data-action="view" data-value="changes">${icon('back')} Changes</button><button class="secondary" data-action="review-jump">${i+1} / ${files.length} files ${icon('down')}</button></div><div class="review-body"><div class="eyebrow">${f[1]}</div><h2>${f[0]}</h2><div class="row between review-meta"><small>Unified diff</small><span class="counts"><span class="positive">+${[74,38,16][i]}</span> <span class="negative">−${[18,10,8][i]}</span></span></div><div class="code-box">${snippets[i].map(([kind,txt])=>`<span class="code-line ${kind}">${esc(txt)}</span>`).join('')}</div><button class="secondary full" data-action="review-note">${icon('chat')} ${state.reviewComments[i]?'Edit your comment':'Add a file comment'}</button>${state.reviewComments[i]?`<div class="review-comment"><small>Your draft comment</small><p>${esc(state.reviewComments[i])}</p></div>`:''}<p class="hint">Review at your pace. Your place and draft comments stay here as you move between files.</p></div><div class="review-bottom"><div class="review-progress">${state.reviewed.size} of ${files.length} files reviewed <span>${Math.round(state.reviewed.size/files.length*100)}%</span></div><div class="row"><button class="secondary" data-action="review-prev" aria-label="Previous file" ${i===0?'disabled':''}>${icon('back')}</button><button class="${state.reviewed.has(i)?'secondary':'primary'} grow" data-action="review-mark">${icon('check')}${state.reviewed.has(i)?'Reviewed':'Mark reviewed'}</button><button class="secondary" data-action="review-next" aria-label="${i===2?'Finish review':'Next file'}">${icon(i===2?'check':'chevron')}</button></div></div>`;
}
const oldChanges=renderChanges;
renderChanges=function(){oldChanges();const summary=main.querySelector('.summary');summary.insertAdjacentHTML('afterend',`<div class="review-summary"><span>${state.reviewed.size} of 3 files reviewed</span><button data-action="diff" data-value="0">Continue review ${icon('chevron')}</button></div>`);};
openDiff=function(i){state.reviewIndex=i;state.view='review-file';if(sheet.open)closeSheet();render();};
renderMore=function(){baseMore();main.querySelector('.page').insertAdjacentHTML('afterbegin',`<button class="search-action full" data-action="command-search">${icon('search')} Find an action… <span class="muted">⌘</span></button>`);main.querySelector('.page').insertAdjacentHTML('beforeend',`<div class="section-label">On your phone</div><div class="list">${row('Install Codemux','Home Screen setup for iPhone & Android','phone','install')}${row('Notifications',state.notifications==='enabled'?'Enabled in demo':'Questions, approvals, and finished work','bell','notifications')}${row('Try connection recovery','Keep a draft while reconnecting','wifi','disconnect')}</div>`);};
renderTerminal=function(){baseTerminal();const input=main.querySelector('#command');input.value=state.commandDraft||'';input.addEventListener('input',e=>state.commandDraft=e.target.value);main.querySelector('form').addEventListener('submit',e=>{if(!state.connected){e.preventDefault();e.stopImmediatePropagation();return;}state.commandDraft='';},true);main.querySelector('form button').disabled=!state.connected;main.querySelector('.key-row').insertAdjacentHTML('beforeend',`<button data-action="dismiss-keyboard" aria-label="Dismiss keyboard">${icon('keyboard')}</button>`);};
function commandSearch() {
  const actions=[['All workspaces','home',''],['New workspace','new',''],['Review changes','view','changes'],['Find a file','view','files'],['Open terminal','view','terminal'],['Switch agent or session','sessions',''],['Browser preview','view-preview',''],['Tasks & agents','tasks',''],['Pull requests','pr',''],['Settings','settings',''],['Install on your phone','install',''],['Notification preferences','notifications','']];
  const entries=[...actions,...workspaces.map(([name])=>[name,'workspace',name])];
  const rows=q=>entries.filter(a=>a[0].toLowerCase().includes(q.toLowerCase())).map(([label,action,value])=>actionButton(label,action,'chevron',value)).join('')||'<p class="hint">No matching actions in this concept.</p>';
  openSheet('Find an action',`<div class="search-wrap">${icon('search')}<input class="input" id="action-search" aria-label="Search actions" placeholder="Search tools, workspaces, settings…"></div><div id="action-results" class="list">${rows('')}</div>`);
  document.querySelector('#action-search').addEventListener('input',e=>{document.querySelector('#action-results').innerHTML=rows(e.target.value);icons();});
}
window.handlePrototypeAction=function(a,v,b){
  switch(a){
    case 'home': go('home');return true;
    case 'view': go(v);return true;
    case 'view-preview':go('preview');return true;
    case 'workspace':openWorkspace(v);return true;
    case 'home-filter':state.homeFilter=v;render();return true;
    case 'dismiss-install':state.installDismissed=true;render();return true;
    case 'install':installSheet();return true;
    case 'platform':state.platform=v;installSheet();return true;
    case 'preview-installed':state.installedPreview=true;render();notificationsSheet();return true;
    case 'notifications':notificationsSheet();return true;
    case 'enable-notifications':state.notifications='enabled';render();notificationsSheet();return true;
    case 'disable-notifications':state.notifications='off';render();notificationsSheet();return true;
    case 'block-notifications':state.notifications='blocked';notificationsSheet();return true;
    case 'reset-permission':state.notifications='off';notificationsSheet();return true;
    case 'test-notification':openSheet('Sample notification',`<div class="prototype-notice">In-page preview · not a system notification</div><button class="notification-sample full notification-button" data-action="notification-open"><div class="row between"><span class="eyebrow">CODEMUX</span><small>now</small></div><strong>Codex needs your input</strong><p>Fix reconnect behavior · a question is waiting.</p><span>Open workspace ${icon('chevron')}</span></button><p class="hint">A real notification would take you straight to the relevant workspace.</p>`);return true;
    case 'notification-open':openWorkspace(homeNames[1]);return true;
    case 'later':closeSheet();return true;
    case 'sessions':case 'panes':sessionSheet();return true;
    case 'choose-session':remember();state.session=v;selectedSessions.set(state.workspace,v);restore();state.view='chat';closeSheet();render();return true;
    case 'command-search':commandSearch();return true;
    case 'answer-question':openSheet('One choice before I finish',`<p class="hint">Fix reconnect behavior · Codex</p><h3>When your phone reconnects, what should happen to an unsent draft?</h3><div class="list spaced">${actionButton('Keep it as a draft','question-choice','check','Keep drafts for manual sending')}${actionButton('Ask before sending','question-choice','check','Ask before sending')}</div><p class="hint">This choice only affects the demo conversation.</p>`);return true;
    case 'question-choice':state.approved=true;Object.assign(scenarios[homeNames[1]],{answered:true,status:'review',label:'Ready for review',meta:'Answer sent · review changes'});state.messages.push(v);state.scroll=999999;closeSheet();render();return true;
    case 'queued':openSheet('Queued follow-ups',`<p class="hint">These demo messages wait until the current run finishes.</p><div class="list">${state.queued.map((q,i)=>`<div class="list-row"><span class="grow"><strong>${esc(q)}</strong><small>Queued · not yet sent</small></span><button class="icon-button" data-action="remove-queued" data-value="${i}" aria-label="Remove queued message">${icon('close')}</button></div>`).join('')}</div><button class="primary full spaced" data-action="finish-run">Simulate run finishing</button>`);return true;
    case 'remove-queued':state.queued.splice(Number(v),1);closeSheet();render();return true;
    case 'finish-run':state.messages.push(...state.queued);state.queued=[];state.running=false;closeSheet();render();toast('Queued messages delivered in demo');return true;
    case 'connection':openSheet(state.connected?'Your desktop is connected':'Reconnecting to your desktop',`<div class="summary">${icon('device')}<span class="grow"><strong>Development desktop</strong><small>${state.connected?'Online · sample connection':'Connection interrupted · demo'}</small></span><span class="live-dot ${state.connected?'':'offline'}"></span></div><p class="hint">Agents run on your desktop. Closing the phone app does not stop their work.</p><button class="secondary full" data-action="${state.connected?'disconnect':'restore-connection'}">${state.connected?'Try a connection interruption':'Reconnect in demo'}</button><p class="hint">Try typing a draft first. It stays in place while the connection recovers.</p>`);return true;
    case 'disconnect':remember();state.connected=false;if(sheet.open)closeSheet();render();toast('Demo connection interrupted. Drafts are preserved.');return true;
    case 'restore-connection':state.connected=true;if(sheet.open)closeSheet();render();toast('Reconnected. Your draft is ready when you are.');return true;
    case 'dismiss-keyboard':document.activeElement?.blur();return true;
    case 'review-jump':openSheet('Changed files',`<div class="list">${files.map((f,i)=>actionButton(`${i+1}. ${f[0]}${state.reviewed.has(i)?' · reviewed':''}`,'diff','chevron',i)).join('')}</div>`);return true;
    case 'review-prev':state.reviewIndex=Math.max(0,state.reviewIndex-1);render();return true;
    case 'review-next':if(state.reviewIndex===2)go('changes');else{state.reviewIndex++;render();}return true;
    case 'review-mark':state.reviewed.has(state.reviewIndex)?state.reviewed.delete(state.reviewIndex):state.reviewed.add(state.reviewIndex);render();return true;
    case 'review-note':openSheet('Comment on this file',`<p class="hint">${files[state.reviewIndex][0]} · draft review comment</p><label class="field" for="file-comment">Your comment</label><textarea id="file-comment" class="input" rows="4" placeholder="What should the agent check?">${esc(state.reviewComments[state.reviewIndex]||'')}</textarea><button class="primary full spaced" data-action="save-review-note">Save draft comment</button>`);return true;
    case 'save-review-note':state.reviewComments[state.reviewIndex]=document.querySelector('#file-comment').value;closeSheet();render();return true;
    case 'new':openSheet('Start something new',`<p class="hint">Create a workspace on your connected desktop.</p><label class="field" for="workspace-name">What are you building?</label><input id="workspace-name" class="input" placeholder="e.g. Add search to the dashboard"><div class="form-grid"><div><label class="field" for="project">Project</label><select id="project" class="input"><option>codemux</option><option>fieldnotes</option></select></div><div><label class="field" for="new-agent">Agent</label><select id="new-agent" class="input"><option>Codex</option><option>Claude</option></select></div></div><label class="field" for="workspace-prompt">First instruction</label><textarea id="workspace-prompt" class="input" rows="3" placeholder="Describe what you want to build…"></textarea><button class="primary full spaced" data-action="create-workspace">${icon('plus')} Create & start · demo</button>`);return true;
    case 'create-workspace':{
      const name=document.querySelector('#workspace-name').value.trim();if(!name){document.querySelector('#workspace-name').focus();return true;}
      if(workspaces.some(w=>w[0]===name)){toast('A workspace with that name already exists');return true;}
      const project=document.querySelector('#project').value,prompt=document.querySelector('#workspace-prompt').value,agent=document.querySelector('#new-agent').value;
      workspaces.push([name,'New workspace · demo',project[0].toUpperCase()]);scenarios[name]={status:'new',label:'New workspace',project,branch:'new-workspace',meta:'Ready to start',prompt,answer:'Your new workspace is ready on the demo desktop. You can keep building from here.'};openWorkspace(name);state.model=agent==='Claude'?'Claude · Sonnet':'Codex · GPT-5.4';render();toast('Demo workspace created');return true;
    }
    case 'about':openSheet('Codemux mobile · Concept 02',`<div class="about-copy"><p>A mobile workspace for the things you do at your desk.</p><ul><li>Start from Workspaces, or create a new one.</li><li>Answer a question in <strong>Fix reconnect behavior</strong>.</li><li>Queue a follow-up in <strong>Refresh the homepage</strong>.</li><li>Review files, leave comments, and mark progress.</li><li>Try the Home Screen and notification onboarding.</li></ul><p>All actions use sample data. Installation, permissions, push delivery, agents, Git, and remote connections are not live.</p><p>Production target: <strong>app.codemux.org</strong>. Your desktop continues to run the work.</p><button class="secondary full" data-action="install">Preview phone setup</button></div>`);return true;
  }
  return false;
};
document.addEventListener('change',e=>{if(e.target.matches('[data-notification]'))state['notify_'+e.target.dataset.notification]=e.target.checked;});
render();
