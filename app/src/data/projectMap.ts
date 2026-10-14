import { MarkerType, type Edge, type Node } from '@xyflow/react'

export type ComponentIcon =
  | 'desktop'
  | 'map'
  | 'bot'
  | 'terminal'
  | 'share'
  | 'database'
  | 'shield'
  | 'review'

export type ComponentData = {
  title: string
  kind: string
  summary: string
  technology: string
  status: string
  icon: ComponentIcon
  responsibilities: string[]
  files: string[]
  questions?: string[]
  answers?: Record<string, string>
}

export type LayerData = {
  label: string
  tone: 'blue' | 'violet' | 'teal' | 'amber'
}

export type HeimNode =
  | Node<ComponentData, 'component'>
  | Node<LayerData, 'layer'>

const component = (
  id: string,
  position: { x: number; y: number },
  data: ComponentData,
): HeimNode => ({ id, type: 'component', position, data })

const layer = (
  id: string,
  position: { x: number; y: number },
  width: number,
  height: number,
  data: LayerData,
): HeimNode => ({
  id,
  type: 'layer',
  position,
  data,
  draggable: false,
  selectable: false,
  connectable: false,
  zIndex: -1,
  style: { width, height },
})

export const initialNodes: HeimNode[] = [
  layer('layer-client', { x: 25, y: 25 }, 390, 190, {
    label: 'Client Layer',
    tone: 'blue',
  }),
  layer('layer-agent', { x: 590, y: 25 }, 450, 190, {
    label: 'Agent Layer',
    tone: 'violet',
  }),
  layer('layer-data', { x: 25, y: 470 }, 560, 175, {
    label: 'Project Context',
    tone: 'teal',
  }),
  layer('layer-safety', { x: 650, y: 470 }, 390, 175, {
    label: 'Execution & Safety',
    tone: 'amber',
  }),
  component('heim-ui', { x: 55, y: 85 }, {
    title: 'Heim Workspace',
    kind: 'Desktop UI',
    summary: 'The shared workspace for the developer and AI agent.',
    technology: 'React · TypeScript',
    status: 'In progress',
    icon: 'desktop',
    responsibilities: [
      'Project map and node inspection',
      'Contextual agent conversation',
      'Review and approve proposed changes',
    ],
    files: ['src/App.tsx', 'src/index.css'],
  }),
  component('project-map', { x: 275, y: 85 }, {
    title: 'Project Map',
    kind: 'Shared context',
    summary: 'A living plan that connects goals, decisions, and development work.',
    technology: 'Interactive node graph',
    status: 'Prototype',
    icon: 'map',
    responsibilities: [
      'Keep project intent visible',
      'Connect related components and decisions',
      'Provide a focused context for each conversation',
    ],
    files: ['src/data/projectMap.ts'],
  }),
  component('agent-host', { x: 620, y: 85 }, {
    title: 'Agent Host',
    kind: 'Desktop backend',
    summary: 'Owns agent sessions, permissions, and communication with the UI.',
    technology: 'Tauri · Rust',
    status: 'Planned',
    icon: 'bot',
    responsibilities: [
      'Manage CLI process lifecycle',
      'Normalize agent events',
      'Enforce workspace and approval boundaries',
    ],
    files: ['src-tauri/src/lib.rs'],
  }),
  component('agent-cli', { x: 850, y: 85 }, {
    title: 'Agent CLI',
    kind: 'Development engine',
    summary: 'Performs implementation work under Heim-managed context and policy.',
    technology: 'Provider to be selected',
    status: 'Not connected',
    icon: 'terminal',
    responsibilities: [
      'Read and change project code',
      'Run approved development tasks',
      'Report structured progress and results',
    ],
    files: [],
  }),
  component('mcp-context', { x: 130, y: 275 }, {
    title: 'Context Bridge',
    kind: 'MCP tools',
    summary: 'Gives the agent controlled access to map context and change proposals.',
    technology: 'MCP · Transport TBD',
    status: 'Planned',
    icon: 'share',
    responsibilities: [
      'Read selected node and related decisions',
      'Submit map changes as proposals',
      'Report work status to Heim',
    ],
    files: [],
  }),
  component('sqlite-store', { x: 170, y: 515 }, {
    title: 'Project Store',
    kind: 'Local persistence',
    summary: 'Stores the map, decisions, agent sessions, and review history.',
    technology: 'SQLite',
    status: 'Planned',
    icon: 'database',
    responsibilities: [
      'Persist project map and relationships',
      'Link sessions to relevant nodes',
      'Restore state after app restart',
    ],
    files: [],
  }),
  component('workspace-guard', { x: 680, y: 515 }, {
    title: 'Workspace Guard',
    kind: 'Permission boundary',
    summary: 'Restricts project access and filters sensitive process inputs.',
    technology: 'Rust · OS process controls',
    status: 'Planned',
    icon: 'shield',
    responsibilities: [
      'Validate canonical paths under the opened project',
      'Pass executable and arguments separately',
      'Filter secrets and stop the complete process tree',
    ],
    files: ['src-tauri/src/lib.rs'],
  }),
  component('change-review', { x: 875, y: 515 }, {
    title: 'Change Review',
    kind: 'Approval workflow',
    summary: 'Shows code diffs and gates destructive or external actions.',
    technology: 'Diff · Approval UI',
    status: 'Planned',
    icon: 'review',
    responsibilities: [
      'Review file changes before and after work',
      'Approve or reject risky commands',
      'Undo reviewed changes when safe',
    ],
    files: [],
  }),
]

const edgeStyle = { stroke: '#aeb8c9', strokeWidth: 1.6 }

export const initialEdges: Edge[] = [
  { id: 'ui-map', source: 'heim-ui', target: 'project-map', style: edgeStyle },
  { id: 'map-agent', source: 'project-map', target: 'agent-host', style: edgeStyle },
  {
    id: 'host-cli',
    source: 'agent-host',
    target: 'agent-cli',
    style: edgeStyle,
    markerEnd: { type: MarkerType.ArrowClosed, color: '#aeb8c9' },
  },
  { id: 'host-context', source: 'agent-host', target: 'mcp-context', style: edgeStyle },
  { id: 'context-store', source: 'mcp-context', target: 'sqlite-store', style: edgeStyle },
  { id: 'host-guard', source: 'agent-host', target: 'workspace-guard', style: edgeStyle },
  { id: 'guard-review', source: 'workspace-guard', target: 'change-review', style: edgeStyle },
]
