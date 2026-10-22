import { useMemo, useState, type FormEvent } from 'react'
import {
  Background,
  BackgroundVariant,
  Controls,
  MiniMap,
  ReactFlow,
  useEdgesState,
  useNodesState,
  type Edge,
  type Node,
  type ReactFlowInstance,
} from '@xyflow/react'
import {
  Bot,
  Check,
  ChevronLeft,
  ChevronRight,
  Code2,
  Database,
  FileCode2,
  FolderOpen,
  LayoutGrid,
  ListChecks,
  MessageCircle,
  Plus,
  Send,
  ShieldCheck,
  Workflow,
} from 'lucide-react'
import { QuestionnairePanel } from './QuestionnairePanel'
import type { DecisionQuestion } from './data/projectMap'

type DemoNodeData = { title: string; label: string; summary: string; status: string; responsibilities: string[]; files: string[]; questions: DecisionQuestion[] }
type DemoFlowNode = Node<DemoNodeData, 'default'>
type DemoTab = 'overview' | 'files' | 'tasks' | 'relationships'
type DemoMessage = { id: number; role: 'user' | 'system'; text: string }

const initialNodes: DemoFlowNode[] = [
  { id: 'web', type: 'default', position: { x: 30, y: 80 }, data: { title: 'Web application', label: 'Web application', summary: 'The interface for browsing and managing a personal reading list.', status: 'In progress', responsibilities: ['Display the book collection', 'Let readers update book details'], files: ['src/App.tsx', 'src/components/BookList.tsx'], questions: [] }, style: { width: 220 } },
  { id: 'catalog', type: 'default', position: { x: 360, y: 35 }, data: { title: 'Book catalog', label: 'Book catalog', summary: 'Owns book records, search, and reading status.', status: 'Not started', responsibilities: ['Create and update book records', 'Search and filter the collection'], files: ['src/catalog/books.ts'], questions: [
    { prompt: 'Where should the book collection be stored?', options: [{ label: 'Locally on this device', recommended: true }, { label: 'In a cloud database', recommended: false }] },
    { prompt: 'How should a cover image be added?', options: [{ label: 'Enter an image URL', recommended: false }, { label: 'Upload an image file', recommended: true }] },
  ] }, style: { width: 230 } },
  { id: 'storage', type: 'default', position: { x: 700, y: 35 }, data: { title: 'Local storage', label: 'Local storage', summary: 'Persists the collection between sessions without requiring an account.', status: 'Not started', responsibilities: ['Save and load book records', 'Keep data available offline'], files: ['src/storage/libraryStore.ts'], questions: [] }, style: { width: 230 } },
  { id: 'import', type: 'default', position: { x: 360, y: 285 }, data: { title: 'Import and export', label: 'Import and export', summary: 'Moves a reader’s collection into and out of the application.', status: 'Not started', responsibilities: ['Import a collection file', 'Export a backup'], files: [], questions: [] }, style: { width: 230 } },
]
const initialEdges: Edge[] = [
  { id: 'web-catalog', source: 'web', target: 'catalog', label: 'interacts with', animated: false },
  { id: 'catalog-storage', source: 'catalog', target: 'storage', label: 'depends on', animated: false },
  { id: 'import-catalog', source: 'import', target: 'catalog', label: 'updates', animated: false },
]

const initialTasks = [
  { id: 1, text: 'Define the book record fields', done: true },
  { id: 2, text: 'Choose a persistence strategy', done: false },
  { id: 3, text: 'Add collection search', done: false },
]
const demoRecommendationNodeId = 'catalog'
const demoRecommendationRationale = 'Static preview example: the book catalog defines the records and search behavior needed by the rest of this sample map. This is not an AI recommendation.'

export function DemoWorkspace() {
  const [nodes, setNodes, onNodesChange] = useNodesState<DemoFlowNode>(initialNodes)
  const [edges, , onEdgesChange] = useEdgesState(initialEdges)
  const [selectedNodeId, setSelectedNodeId] = useState('catalog')
  const [activeTab, setActiveTab] = useState<DemoTab>('overview')
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false)
  const [editing, setEditing] = useState(false)
  const [workspaceTab, setWorkspaceTab] = useState<'chat' | 'terminal'>('chat')
  const [answers, setAnswers] = useState<Record<string, string>>({
    'Where should the book collection be stored?': 'Locally on this device',
    'How should a cover image be added?': 'Upload an image file',
  })
  const [tasks, setTasks] = useState(initialTasks)
  const [messages, setMessages] = useState<DemoMessage[]>([{ id: 1, role: 'system', text: 'Demo mode is active. AI and project-file operations are disabled.' }])
  const [message, setMessage] = useState('')
  const [notice, setNotice] = useState('')
  const [saved, setSaved] = useState(true)
  const [flow, setFlow] = useState<ReactFlowInstance<DemoFlowNode, Edge> | null>(null)
  const selectedNode = nodes.find((node) => node.id === selectedNodeId) ?? nodes[0]
  const selectedData = selectedNode?.data
  const displayedNodes = useMemo(() => nodes.map((node) => node.id === demoRecommendationNodeId
    ? { ...node, className: 'demo-next-node', ariaLabel: `${node.data.title}. NEXT (static demo recommendation): ${demoRecommendationRationale}` }
    : node), [nodes])
  const unansweredCount = useMemo(() => (selectedData?.questions ?? []).filter((question) => !(answers[question.prompt] ?? '').trim()).length, [answers, selectedData])

  const updateSelected = (changes: Partial<DemoNodeData>) => {
    setNodes((current) => current.map((node) => node.id === selectedNodeId ? { ...node, data: { ...node.data, ...changes, label: changes.title ?? node.data.title } } : node))
    setSaved(false)
  }

  const addNode = () => {
    const id = `component-${Date.now()}`
    const newNode: DemoFlowNode = {
      id,
      type: 'default',
      position: { x: 300 + Math.random() * 120, y: 400 + Math.random() * 100 },
      data: { title: 'New component', label: 'New component', summary: 'Describe this component’s responsibility and boundaries.', status: 'Not started', responsibilities: ['Add a responsibility'], files: [], questions: [] },
      style: { width: 220 },
    }
    setNodes((current) => [...current, newNode])
    setSelectedNodeId(id)
    setActiveTab('overview')
    setSaved(false)
  }

  const sendMessage = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const text = message.trim()
    if (!text) return
    setMessages((current) => [...current, { id: Date.now(), role: 'user', text }, { id: Date.now() + 1, role: 'system', text: 'Not sent: AI is disabled in this UI preview.' }])
    setMessage('')
  }

  const previewAction = (name: string) => setNotice(`${name} is a visual demo action; no project data is changed.`)

  return (
    <main className={`demo-app${sidebarCollapsed ? ' is-sidebar-collapsed' : ''}`}>
      {!sidebarCollapsed && <aside className="demo-sidebar">
        <div className="demo-brand"><div className="demo-brand-icon"><Workflow size={18} /></div><strong>Heim</strong><span className="demo-pill">UI DEMO</span></div>
        <div className="demo-project-label">WORKSPACE</div>
        <button className="demo-project-button" type="button" onClick={() => previewAction('Project switcher')}><FolderOpen size={15} /><span>My Reading List</span><span>⌄</span></button>
        <nav className="demo-nav" aria-label="Demo workspace navigation">
          <button className="is-active" type="button" onClick={() => setNotice('Architecture map is the current demo view.')}><Workflow size={16} />Architecture map</button>
          <button type="button" onClick={() => { setActiveTab('tasks'); setNotice('Development tasks opened in the details panel.') }}><ListChecks size={16} />Development tasks</button>
          <button type="button" onClick={() => previewAction('Activity')}><MessageCircle size={16} />Activity</button>
        </nav>
        <div className="demo-sidebar-footer"><ShieldCheck size={14} /> Sample data only</div>
      </aside>}

      <section className="demo-main">
        <header className="demo-toolbar">
          {sidebarCollapsed && <button className="demo-icon-button" type="button" aria-label="Expand sidebar" onClick={() => setSidebarCollapsed(false)}><ChevronRight size={17} /></button>}
          <button className="demo-sidebar-toggle" type="button" aria-label="Collapse sidebar" onClick={() => setSidebarCollapsed((value) => !value)}><ChevronLeft size={17} /></button>
          <div className="demo-heading"><span className="demo-heading-icon"><Workflow size={18} /></span><div><strong>My Reading List</strong><small>Architecture map · 4 components</small></div></div>
          <div className="demo-toolbar-actions">
            <button className="button button-development" type="button" disabled title="AI is disabled in UI demo"><Code2 size={14} />Develop</button>
            <button className="button button-outline" type="button" onClick={addNode}><Plus size={14} />Add node</button>
            <button className="button button-outline" type="button" onClick={() => { setSaved(true); setNotice('Demo map saved in temporary page state.') }}>{saved ? <Check size={14} /> : <span className="demo-unsaved-dot" />}Save map</button>
            <button className="button button-subtle" type="button" onClick={() => flow?.fitView({ duration: 250, padding: 0.15 })}><LayoutGrid size={15} />Fit</button>
          </div>
        </header>

        <div className="demo-map-canvas">
          <div className="demo-canvas-note"><span className="demo-status-dot" />{saved ? 'All changes saved' : 'Unsaved changes'}<span>·</span> Sample architecture · NEXT is a static demo example</div>
          <ReactFlow
            nodes={displayedNodes}
            edges={edges}
            onNodesChange={(changes) => { onNodesChange(changes); if (changes.some((change) => change.type === 'position')) setSaved(false) }}
            onEdgesChange={onEdgesChange}
            onNodeClick={(_, node) => { setSelectedNodeId(node.id); setActiveTab('overview') }}
            onInit={setFlow}
            fitView
            fitViewOptions={{ padding: 0.18 }}
            minZoom={0.25}
            maxZoom={1.5}
            proOptions={{ hideAttribution: true }}
          >
            <Background variant={BackgroundVariant.Dots} gap={18} size={1} color="#dce2ec" />
            <Controls showInteractive={false} />
            <MiniMap pannable zoomable nodeColor={(node) => node.id === selectedNodeId ? '#c8d8fb' : '#e5eaf2'} />
          </ReactFlow>
        </div>

        <section className="demo-agent-panel">
          <header className="demo-agent-tabs">
            <div>
              <button className={workspaceTab === 'chat' ? 'is-active' : ''} type="button" onClick={() => setWorkspaceTab('chat')}><MessageCircle size={14} />Chat</button>
              <button className={workspaceTab === 'terminal' ? 'is-active' : ''} type="button" onClick={() => setWorkspaceTab('terminal')}><Code2 size={14} />Terminal</button>
            </div>
            <span className="demo-ai-disabled"><Bot size={13} />AI disabled in preview</span>
          </header>
          {workspaceTab === 'chat' ? <div className="demo-chat-content">
            <div className="demo-messages" aria-live="polite">{messages.map((item) => <div className={`demo-message is-${item.role}`} key={item.id}><span>{item.role === 'user' ? 'You' : 'Demo'}</span><p>{item.text}</p></div>)}</div>
            <form className="demo-composer" onSubmit={sendMessage}><input value={message} onChange={(event) => setMessage(event.target.value)} placeholder="Type here to test the composer (not sent to AI)…" /><button className="send-button" type="submit" aria-label="Add demo message"><Send size={14} /></button></form>
          </div> : <div className="demo-terminal"><Code2 size={16} />Terminal is inactive in UI preview. No commands are run.</div>}
        </section>
      </section>

      <aside className="details-panel demo-details" aria-label="Selected component details">
        <div className="details-summary">
          <div className="details-icon"><Database size={21} /></div>
          <div className="details-title-copy"><h2>{selectedData?.title ?? 'Select a component'}</h2><span className="status-badge"><span />{selectedData?.status ?? ''}</span></div>
          <button className="button button-outline" type="button" onClick={() => setEditing((value) => !value)}>{editing ? 'Done' : 'Edit'}</button>
        </div>
        <div className="detail-tabs" role="tablist" aria-label="Component details">
          {(['overview', 'files', 'tasks', 'relationships'] as const).map((tab) => <button className={`detail-tab${activeTab === tab ? ' is-active' : ''}`} role="tab" aria-selected={activeTab === tab} type="button" key={tab} onClick={() => setActiveTab(tab)}>{tab[0].toUpperCase() + tab.slice(1)}</button>)}
        </div>
        <div className="detail-scroll-area">
          {activeTab === 'overview' && selectedData && <div className="detail-section">
            {selectedNode.id === demoRecommendationNodeId && <p className="demo-next-rationale"><strong>NEXT · Static demo example</strong><br />{demoRecommendationRationale}</p>}
            {editing && <div className="demo-edit-fields"><label>Component name<input value={selectedData.title} onChange={(event) => updateSelected({ title: event.target.value })} /></label><label>Responsibility and scope<textarea rows={4} value={selectedData.summary} onChange={(event) => updateSelected({ summary: event.target.value })} /></label><label>Status<select value={selectedData.status} onChange={(event) => updateSelected({ status: event.target.value })}><option>Not started</option><option>In progress</option><option>Completed</option></select></label></div>}
            <h3>Responsibility and scope</h3><p className="detail-summary-text">{selectedData.summary}</p>
            <QuestionnairePanel questions={selectedData.questions} answers={answers} onAnswer={(prompt, answer) => { setAnswers((current) => ({ ...current, [prompt]: answer })); setSaved(false) }} />
            {selectedData.questions.length > 0 && <p className="demo-question-gate">{unansweredCount > 0 ? `${unansweredCount} open question${unansweredCount === 1 ? '' : 's'} · development stays locked` : 'All questions answered · ready to save'}</p>}
            <h3 className="section-heading-spaced">Key responsibilities</h3><ul className="responsibility-list">{selectedData.responsibilities.map((item) => <li key={item}>{item}</li>)}</ul>
            <section className="demo-plan-card"><div><ListChecks size={15} /><strong>Unit goal and completion criteria</strong></div><p>Build the first usable book collection view.</p><small>Demo plan · AI is not used</small><button className="button button-outline" type="button" disabled={unansweredCount > 0} onClick={() => previewAction('Develop selected node')}>Develop selected node</button></section>
            <h3 className="section-heading-spaced">Details</h3><dl className="detail-facts"><div><dt>Type</dt><dd>System responsibility</dd></div><div><dt>Technology</dt><dd>React · TypeScript</dd></div><div><dt>Files</dt><dd>{selectedData.files.length ? `${selectedData.files.length} linked` : 'None linked'}</dd></div></dl>
          </div>}
          {activeTab === 'files' && <div className="detail-section"><h3>Linked files</h3>{selectedData?.files.length ? selectedData.files.map((file) => <div className="file-row" key={file}><FileCode2 size={15} /><span>{file}</span></div>) : <p className="muted-copy">No files linked yet.</p>}<button className="button button-outline demo-action-button" type="button" onClick={() => previewAction('Add file link')}>Add file link</button></div>}
          {activeTab === 'tasks' && <div className="detail-section"><h3>Development tasks</h3>{tasks.map((task) => <button className="demo-task-row" type="button" key={task.id} onClick={() => setTasks((current) => current.map((item) => item.id === task.id ? { ...item, done: !item.done } : item))}><span className={task.done ? 'is-done' : ''}>{task.done && <Check size={12} />}</span>{task.text}</button>)}</div>}
          {activeTab === 'relationships' && <div className="detail-section"><h3>Relationships</h3>{edges.filter((edge) => edge.source === selectedNodeId || edge.target === selectedNodeId).map((edge) => { const relatedId = edge.source === selectedNodeId ? edge.target : edge.source; const related = nodes.find((node) => node.id === relatedId); return <button className="demo-relationship" type="button" key={edge.id} onClick={() => { setSelectedNodeId(relatedId); setActiveTab('overview') }}><Workflow size={15} /><span>{related?.data.title ?? relatedId}</span><small>{edge.label}</small></button> })}</div>}
        </div>
      </aside>
      {notice && <div className="demo-toast" role="status"><span>{notice}</span><button type="button" aria-label="Dismiss notice" onClick={() => setNotice('')}>×</button></div>}
      <div className="demo-preview-badge">Sample UI · changes are temporary · AI disabled</div>
    </main>
  )
}
