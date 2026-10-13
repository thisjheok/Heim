import { useEffect, useMemo, useRef, useState, type FormEvent, type KeyboardEvent } from 'react'
import {
  addEdge,
  Background,
  BackgroundVariant,
  Controls,
  Handle,
  MiniMap,
  Position,
  ReactFlow,
  useEdgesState,
  useNodesState,
  type Connection,
  type Edge,
  type Node,
  type NodeProps,
  type ReactFlowInstance,
} from '@xyflow/react'
import {
  Activity,
  Bot,
  Boxes,
  Check,
  ChevronDown,
  ChevronRight,
  CircleHelp,
  ClipboardList,
  Code2,
  Database,
  FileCode2,
  FileText,
  Folder,
  FolderOpen,
  GitBranch,
  LayoutGrid,
  Link2,
  ListChecks,

  MessageCircle,
  Monitor,
  MoreHorizontal,
  PanelRightClose,
  Plus,
  Search,
  Send,
  Settings2,
  ShieldCheck,
  Sparkles,
  SquareTerminal,
  Workflow,
  X,
  type LucideIcon,
} from 'lucide-react'
import {
  initialEdges,
  initialNodes,
  type ComponentData,
  type ComponentIcon,
  type HeimNode,
  type LayerData,
} from './data/projectMap'
import '@xyflow/react/dist/style.css'
import './App.css'

type ComponentFlowNode = Node<ComponentData, 'component'>
type LayerFlowNode = Node<LayerData, 'layer'>
type WorkspaceTab = 'chat' | 'tasks' | 'context' | 'terminal'
type DetailTab = 'overview' | 'files' | 'tasks' | 'relationships'
type ChatMessage = { id: number; text: string; time: string }

const iconByName: Record<ComponentIcon, LucideIcon> = {
  desktop: Monitor,
  map: Workflow,
  bot: Bot,
  terminal: SquareTerminal,
  share: Link2,
  database: Database,
  shield: ShieldCheck,
  review: ClipboardList,
}

function ComponentNodeView({ data, selected }: NodeProps<ComponentFlowNode>) {
  const Icon = iconByName[data.icon]

  return (
    <div className={`component-node${selected ? ' is-selected' : ''}`}>
      <Handle type="target" position={Position.Left} />
      <div className="component-node-icon">
        <Icon size={18} strokeWidth={1.8} />
      </div>
      <div className="component-node-copy">
        <strong>{data.title}</strong>
        <span>{data.kind}</span>
        <small>{data.technology}</small>
      </div>
      <span className={`node-status-dot ${data.status === 'In progress' ? 'is-active' : ''}`} />
      <Handle type="source" position={Position.Right} />
    </div>
  )
}

function LayerNodeView({ data }: NodeProps<LayerFlowNode>) {
  return (
    <div className={`layer-node layer-${data.tone}`}>
      <span>{data.label}</span>
    </div>
  )
}

const nodeTypes = {
  component: ComponentNodeView,
  layer: LayerNodeView,
}

const navItems = [
  { label: 'Overview', caption: 'Project overview', icon: Activity },
  { label: 'Project Map', caption: 'Visualize architecture', icon: Workflow },
  { label: 'Components', caption: 'Manage components', icon: Boxes },
  { label: 'Docs', caption: 'Architecture & notes', icon: FileText },
  { label: 'Agents', caption: 'AI collaborators', icon: Bot },
]

const workspaceTabs: { id: WorkspaceTab; label: string; icon: LucideIcon }[] = [
  { id: 'chat', label: 'Chat', icon: MessageCircle },
  { id: 'tasks', label: 'Tasks', icon: ListChecks },
  { id: 'context', label: 'Context', icon: FileCode2 },
  { id: 'terminal', label: 'Terminal', icon: SquareTerminal },
]

const detailTabs: { id: DetailTab; label: string }[] = [
  { id: 'overview', label: 'Overview' },
  { id: 'files', label: 'Files' },
  { id: 'tasks', label: 'Tasks' },
  { id: 'relationships', label: 'Relationships' },
]

function App() {
  const [nodes, setNodes, onNodesChange] = useNodesState<HeimNode>(initialNodes)
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>(initialEdges)
  const [selectedNodeId, setSelectedNodeId] = useState('project-map')
  const [activeWorkspaceTab, setActiveWorkspaceTab] = useState<WorkspaceTab>('chat')
  const [activeDetailTab, setActiveDetailTab] = useState<DetailTab>('overview')
  const [searchText, setSearchText] = useState('')
  const [draft, setDraft] = useState('')
  const [messages, setMessages] = useState<ChatMessage[]>([])
  const [notice, setNotice] = useState('')
  const [projectTreeOpen, setProjectTreeOpen] = useState(true)
  const [appTreeOpen, setAppTreeOpen] = useState(true)
  const [flow, setFlow] = useState<ReactFlowInstance<HeimNode, Edge> | null>(null)
  const noticeTimer = useRef<number | undefined>(undefined)
  const searchInput = useRef<HTMLInputElement>(null)

  useEffect(() => {
    const focusSearch = (event: globalThis.KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault()
        searchInput.current?.focus()
      }
    }

    window.addEventListener('keydown', focusSearch)
    return () => window.removeEventListener('keydown', focusSearch)
  }, [])

  const selectedNode = nodes.find(
    (node): node is ComponentFlowNode =>
      node.id === selectedNodeId && node.type === 'component',
  )

  const relatedNodes = useMemo(() => {
    const relatedIds = new Set(
      edges
        .filter((edge) => edge.source === selectedNodeId || edge.target === selectedNodeId)
        .flatMap((edge) => [edge.source, edge.target])
        .filter((id) => id !== selectedNodeId),
    )

    return nodes.filter(
      (node): node is ComponentFlowNode =>
        relatedIds.has(node.id) && node.type === 'component',
    )
  }, [edges, nodes, selectedNodeId])

  const showNotice = (message: string) => {
    setNotice(message)
    window.clearTimeout(noticeTimer.current)
    noticeTimer.current = window.setTimeout(() => setNotice(''), 4200)
  }

  const handleSearch = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key !== 'Enter' || !searchText.trim()) return

    const query = searchText.trim().toLowerCase()
    const match = nodes.find(
      (node) =>
        node.type === 'component' &&
        [node.data.title, node.data.kind, node.data.summary, ...node.data.files]
          .join(' ')
          .toLowerCase()
          .includes(query),
    )

    if (!match) {
      showNotice('No matching component or file was found in this map.')
      return
    }

    setSelectedNodeId(match.id)
    flow?.fitView({ nodes: [{ id: match.id }], duration: 350, padding: 0.55 })
  }

  const addNode = () => {
    const id = `new-node-${Date.now()}`
    const addedNodeCount = nodes.filter((node) => node.type === 'component').length - 8
    const offset = Math.max(0, addedNodeCount)
    const nextNode: ComponentFlowNode = {
      id,
      type: 'component',
      position: { x: 430 + (offset % 3) * 35, y: 340 + Math.floor(offset / 3) * 35 },
      data: {
        title: 'New component',
        kind: 'Proposed component',
        summary: 'Describe what this part of the project is responsible for.',
        technology: 'Not decided',
        status: 'Draft',
        icon: 'map',
        responsibilities: ['Define the goal and responsibilities'],
        files: [],
      },
    }

    setNodes((current) => [...current, nextNode])
    setSelectedNodeId(id)
    setActiveDetailTab('overview')
    showNotice('This node is only in the UI preview and is not saved yet.')
  }

  const handleConnect = (connection: Connection) => {
    setEdges((current) => addEdge(connection, current))
  }

  const sendMessage = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const text = draft.trim()
    if (!text) return

    setMessages((current) => [
      ...current,
      {
        id: Date.now(),
        text,
        time: new Intl.DateTimeFormat(undefined, {
          hour: 'numeric',
          minute: '2-digit',
        }).format(new Date()),
      },
    ])
    setDraft('')
    showNotice('Agent CLI is not connected. This message was not sent and is only shown in this preview.')
  }

  const handleComposerKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
      event.preventDefault()
      event.currentTarget.form?.requestSubmit()
    }
  }

  const clearChat = () => {
    setMessages([])
    setDraft('')
    showNotice('Local preview messages cleared.')
  }

  const activeNodeData = selectedNode?.data

  return (
    <main className="app-shell">
      <header className="topbar">
        <div className="brand-area">
          <div className="brand-mark"><Workflow size={19} strokeWidth={2.4} /></div>
          <span className="brand-name">Heim</span>
        </div>

        <div className="topbar-project">
          <button className="project-switcher" type="button" aria-label="Current project" disabled title="Project selection is not available in this preview">
            <span className="project-name">Heim Desktop</span>
            <ChevronDown size={15} />
            <span className="project-stage">Early development</span>
          </button>
        </div>

        <label className="global-search">
          <Search size={17} />
          <input
            ref={searchInput}
            aria-label="Search project map"
            placeholder="Search components, files, or ask anything..."
            value={searchText}
            onChange={(event) => setSearchText(event.target.value)}
            onKeyDown={handleSearch}
          />
          <kbd>⌘ K</kbd>
        </label>

        <div className="topbar-actions">
          <button className="button button-subtle share-button" type="button" disabled title="Team sharing is not part of this prototype">
            <Link2 size={15} /> Share
          </button>
          <button className="button button-primary deploy-button" type="button" disabled title="Agent execution is not connected yet">
            <Sparkles size={15} /> Start building
          </button>
          <button className="avatar-button" type="button" aria-label="User profile">J</button>
        </div>
      </header>

      <div className="workspace">
        <aside className="sidebar">
          <nav className="primary-nav" aria-label="Main navigation">
            {navItems.map(({ label, caption, icon: Icon }) => (
              <button
                className={`nav-item${label === 'Project Map' ? ' is-active' : ''}`}
                type="button"
                key={label}
                disabled={label !== 'Project Map'}
                title={label === 'Project Map' ? undefined : 'Available in a later development stage'}
              >
                <Icon size={18} strokeWidth={1.8} />
                <span className="nav-copy">
                  <strong>{label}</strong>
                  <small>{caption}</small>
                </span>
              </button>
            ))}
          </nav>

          <div className="project-tree">
            <div className="tree-heading">
              <span>PROJECT</span>
              <ChevronDown size={14} />
            </div>
            <button className="tree-row tree-root" type="button" onClick={() => setProjectTreeOpen((open) => !open)}>
              {projectTreeOpen ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
              {projectTreeOpen ? <FolderOpen size={16} /> : <Folder size={16} />}
              <span>heim</span>
            </button>
            {projectTreeOpen && (
              <div className="tree-children">
                <button className="tree-row" type="button" onClick={() => setAppTreeOpen((open) => !open)}>
                  {appTreeOpen ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
                  {appTreeOpen ? <FolderOpen size={16} /> : <Folder size={16} />}
                  <span>app</span>
                </button>
                {appTreeOpen && (
                  <div className="tree-children tree-children-nested">
                    <div className="tree-row tree-static"><ChevronRight size={13} /><Folder size={15} /><span>src</span></div>
                    <div className="tree-row tree-static"><ChevronRight size={13} /><Folder size={15} /><span>src-tauri</span></div>
                  </div>
                )}
                <div className="tree-row tree-static"><ChevronRight size={13} /><Folder size={16} /><span>docs</span></div>
                <div className="tree-row tree-static"><ChevronRight size={13} /><Folder size={16} /><span>assets</span></div>
              </div>
            )}
          </div>

          <div className="sidebar-footer">
            <div className="connection-indicator"><span /> UI preview · Agent offline</div>
            <button className="footer-action" type="button" disabled title="Settings are not available in this prototype">
              <Settings2 size={16} /> Settings
            </button>
            <button className="footer-action" type="button" disabled title="Help is not available in this prototype">
              <CircleHelp size={16} /> Help & feedback
            </button>
          </div>
        </aside>

        <section className="main-column" aria-label="Project map and agent workspace">
          <div className="map-toolbar">
            <div className="map-heading-icon"><Workflow size={21} /></div>
            <div className="map-heading-copy">
              <h1>Project Map</h1>
              <p>Explore the shared plan and discuss each part with your agent.</p>
            </div>
            <div className="map-toolbar-actions">
              <button className="button button-outline" type="button" onClick={addNode}>
                <Plus size={16} /> Add node
              </button>
              <button
                className="button button-subtle fit-button"
                type="button"
                onClick={() => flow?.fitView({ duration: 350, padding: 0.08 })}
                title="Fit map to view"
              >
                <LayoutGrid size={16} /> <span>Fit map</span>
              </button>
              <button className="icon-button more-button" type="button" onClick={() => showNotice('Map options will be available as editing features are added.')} aria-label="More map options">
                <MoreHorizontal size={19} />
              </button>
            </div>
          </div>

          <div className="map-canvas">
            <ReactFlow<HeimNode, Edge>
              nodes={nodes}
              edges={edges}
              nodeTypes={nodeTypes}
              onNodesChange={onNodesChange}
              onEdgesChange={onEdgesChange}
              onConnect={handleConnect}
              onNodeClick={(_, node) => {
                if (node.type === 'component') {
                  setSelectedNodeId(node.id)
                  setActiveDetailTab('overview')
                }
              }}
              onInit={setFlow}
              fitView
              fitViewOptions={{ padding: 0.07 }}
              minZoom={0.25}
              maxZoom={1.5}
              deleteKeyCode={null}
              nodesConnectable
              elementsSelectable
              proOptions={{ hideAttribution: false }}
            >
              <Background variant={BackgroundVariant.Dots} gap={18} size={1} color="#dce2ec" />
              <Controls showInteractive={false} />
              <MiniMap
                pannable
                zoomable
                nodeColor={(node) => node.type === 'layer' ? '#e6eaf2' : '#dce7ff'}
                maskColor="rgba(248, 250, 253, 0.72)"
              />
            </ReactFlow>
            <div className="preview-watermark">Sample project map</div>
          </div>

          <section className="agent-panel" aria-label="Agent workspace">
            <div className="agent-panel-header">
              <div className="workspace-tabs" role="tablist" aria-label="Workspace panels">
                {workspaceTabs.map(({ id, label, icon: Icon }) => (
                  <button
                    type="button"
                    role="tab"
                    aria-selected={activeWorkspaceTab === id}
                    className={`workspace-tab${activeWorkspaceTab === id ? ' is-active' : ''}`}
                    key={id}
                    onClick={() => setActiveWorkspaceTab(id)}
                  >
                    <Icon size={15} /> {label}
                  </button>
                ))}
              </div>
              <div className="agent-panel-actions">
                <span className="selected-context-label"><Workflow size={15} /> Talking about <strong>{activeNodeData?.title ?? 'Project'}</strong></span>
                <span className="agent-offline"><span /> Not connected</span>
                {activeWorkspaceTab === 'chat' && (
                  <button className="text-action" type="button" onClick={clearChat} disabled={messages.length === 0}>
                    <X size={14} /> Clear
                  </button>
                )}
                <button className="icon-button compact-icon-button" type="button" onClick={() => showNotice('Panel options are not available in this preview.')} aria-label="Workspace options">
                  <MoreHorizontal size={17} />
                </button>
              </div>
            </div>

            {activeWorkspaceTab === 'chat' && (
              <div className="chat-panel">
                <div className="chat-messages" aria-live="polite">
                  {messages.length === 0 ? (
                    <div className="chat-empty-state">
                      <div className="empty-state-icon"><Sparkles size={17} /></div>
                      <div>
                        <strong>Discuss this part of your project</strong>
                        <p>Select a map node to give your agent focused context.</p>
                      </div>
                      <span className="preview-note">Agent connection not configured</span>
                    </div>
                  ) : messages.map((message) => (
                    <article className="chat-message user-message" key={message.id}>
                      <div className="message-avatar">J</div>
                      <div className="message-body">
                        <div className="message-meta"><strong>You</strong><time>{message.time}</time></div>
                        <p>{message.text}</p>
                        <span className="message-delivery">Local preview only · not sent to an agent</span>
                      </div>
                    </article>
                  ))}
                </div>
                <form className="composer" onSubmit={sendMessage}>
                  <button className="composer-attach" type="button" disabled title="Attachments are not available yet">
                    <Plus size={17} />
                  </button>
                  <textarea
                    aria-label="Message the selected project component"
                    placeholder={`Message ${activeNodeData?.title ?? 'the project'}... (⌘ ↵ to send)`}
                    value={draft}
                    rows={1}
                    onChange={(event) => setDraft(event.target.value)}
                    onKeyDown={handleComposerKeyDown}
                  />
                  <button className="send-button" type="submit" disabled={!draft.trim()} aria-label="Send message">
                    <Send size={17} />
                  </button>
                </form>
              </div>
            )}

            {activeWorkspaceTab === 'tasks' && (
              <div className="workspace-content task-list">
                <div className="workspace-content-heading"><ListChecks size={17} /><strong>Responsibilities for {activeNodeData?.title ?? 'this project'}</strong></div>
                {(activeNodeData?.responsibilities ?? []).map((task) => (
                  <div className="task-row" key={task}><span className="task-checkbox" /><span>{task}</span><small>Not started</small></div>
                ))}
                {!activeNodeData && <p className="muted-copy">Select a component to see its responsibilities.</p>}
              </div>
            )}

            {activeWorkspaceTab === 'context' && (
              <div className="workspace-content context-content">
                <div className="workspace-content-heading"><FileCode2 size={17} /><strong>Context for this conversation</strong></div>
                <div className="context-card"><span>SELECTED COMPONENT</span><strong>{activeNodeData?.title ?? 'No component selected'}</strong><p>{activeNodeData?.summary ?? 'Choose a node on the map to scope the conversation.'}</p></div>
                <div className="context-card context-card-inline"><span>RELATED COMPONENTS</span><strong>{relatedNodes.length ? relatedNodes.map((node) => node.data.title).join(' · ') : 'None selected'}</strong></div>
                <p className="context-footnote">Context is shown for the UI prototype. It is not yet sent to an agent.</p>
              </div>
            )}

            {activeWorkspaceTab === 'terminal' && (
              <div className="workspace-content terminal-content">
                <div className="terminal-heading"><SquareTerminal size={17} /><strong>Agent terminal</strong><span className="terminal-state">Not running</span></div>
                <p>No process has been started. Heim will only run an agent after its workspace and execution permissions are configured.</p>
              </div>
            )}
          </section>
        </section>

        <aside className="details-panel" aria-label="Selected component details">
          <div className="details-summary">
            <div className="details-icon">{activeNodeData ? (() => { const Icon = iconByName[activeNodeData.icon]; return <Icon size={23} /> })() : <Workflow size={23} />}</div>
            <div className="details-title-copy">
              <h2>{activeNodeData?.title ?? 'Project Map'}</h2>
              <span className="status-badge"><span />{activeNodeData?.status ?? 'No selection'}</span>
            </div>
            <button className="icon-button" type="button" onClick={() => showNotice('Component options are not available in this preview.')} aria-label="Component options"><MoreHorizontal size={18} /></button>
          </div>

          <div className="detail-tabs" role="tablist" aria-label="Component details">
            {detailTabs.map(({ id, label }) => (
              <button
                className={`detail-tab${activeDetailTab === id ? ' is-active' : ''}`}
                type="button"
                role="tab"
                aria-selected={activeDetailTab === id}
                key={id}
                onClick={() => setActiveDetailTab(id)}
              >
                {label}
              </button>
            ))}
          </div>

          <div className="detail-scroll-area">
            {activeDetailTab === 'overview' && activeNodeData && (
              <div className="detail-section">
                <h3>Summary</h3>
                <p className="detail-summary-text">{activeNodeData.summary}</p>
                <h3 className="section-heading-spaced">Key responsibilities</h3>
                <ul className="responsibility-list">
                  {activeNodeData.responsibilities.map((item) => <li key={item}>{item}</li>)}
                </ul>
                <h3 className="section-heading-spaced">Details</h3>
                <dl className="detail-facts">
                  <div><dt>Type</dt><dd><span className="type-pill">{activeNodeData.kind}</span></dd></div>
                  <div><dt>Technology</dt><dd>{activeNodeData.technology}</dd></div>
                  <div><dt>Status</dt><dd><span className="inline-status"><span />{activeNodeData.status}</span></dd></div>
                  <div><dt>Files</dt><dd>{activeNodeData.files.length ? `${activeNodeData.files.length} linked` : 'None linked'}</dd></div>
                </dl>
                <div className="relationships-heading"><h3>Relationships</h3><button type="button" className="text-action" onClick={() => setActiveDetailTab('relationships')}>View all</button></div>
                <RelationshipList relatedNodes={relatedNodes} onSelect={(id) => { setSelectedNodeId(id); setActiveDetailTab('overview') }} />
              </div>
            )}

            {activeDetailTab === 'files' && (
              <div className="detail-section">
                <h3>Linked files</h3>
                {activeNodeData?.files.length ? activeNodeData.files.map((file) => (
                  <div className="file-row" key={file}><Code2 size={16} /><span>{file}</span><ChevronRight size={14} /></div>
                )) : <div className="detail-empty"><FileCode2 size={20} /><strong>No files linked yet</strong><p>Files can be connected to a map component as development progresses.</p></div>}
              </div>
            )}

            {activeDetailTab === 'tasks' && (
              <div className="detail-section">
                <h3>Development tasks</h3>
                {(activeNodeData?.responsibilities ?? []).map((task) => (
                  <div className="detail-task" key={task}><span className="task-checkbox" /><div><strong>{task}</strong><small>Not started</small></div></div>
                ))}
                {!activeNodeData && <div className="detail-empty"><ListChecks size={20} /><strong>Select a component</strong><p>Task details are linked to individual map nodes.</p></div>}
              </div>
            )}

            {activeDetailTab === 'relationships' && (
              <div className="detail-section">
                <h3>Connected components</h3>
                <RelationshipList relatedNodes={relatedNodes} onSelect={(id) => { setSelectedNodeId(id); setActiveDetailTab('overview') }} />
                {relatedNodes.length === 0 && <div className="detail-empty"><GitBranch size={20} /><strong>No relationships</strong><p>Connect this component to another node on the map.</p></div>}
              </div>
            )}
          </div>

          <div className="details-footer"><PanelRightClose size={15} /><span>Details are scoped to the selected map node</span></div>
        </aside>
      </div>

      {notice && <div className="toast-notice" role="status"><Check size={16} />{notice}<button type="button" onClick={() => setNotice('')} aria-label="Dismiss notification"><X size={15} /></button></div>}
    </main>
  )
}

function RelationshipList({ relatedNodes, onSelect }: { relatedNodes: ComponentFlowNode[]; onSelect: (id: string) => void }) {
  if (relatedNodes.length === 0) {
    return <p className="muted-copy">No connected components yet.</p>
  }

  return (
    <div className="relationship-list">
      {relatedNodes.map((node) => {
        const Icon = iconByName[node.data.icon]
        return (
          <button type="button" className="relationship-row" key={node.id} onClick={() => onSelect(node.id)}>
            <span className="relationship-icon"><Icon size={15} /></span>
            <span className="relationship-name">{node.data.title}</span>
            <span className="relationship-type">Related</span>
          </button>
        )
      })}
    </div>
  )
}

export default App
