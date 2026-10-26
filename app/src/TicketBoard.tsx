import { useEffect, useState, type FormEvent } from 'react'
import {
  ArrowDown,
  ArrowUp,
  Check,
  Code2,
  ListChecks,
  LoaderCircle,
  Pencil,
  Plus,
  RotateCcw,
  Sparkles,
  Trash2,
} from 'lucide-react'
import type { HeimNode } from './data/projectMap'
import {
  reviewDecisionLabels,
  runStateLabels,
  ticketIsRunning,
  ticketStatusText,
  type Feature,
  type Ticket,
  type TicketBoardData,
  type TicketInput,
  type TicketReviewDecisionKind,
  type TicketRunReview,
} from './data/tickets'

function parsePersistedTimestamp(value: string | null | undefined): Date | null {
  if (!value) return null
  const iso = value.replace(' ', 'T')
  const timestamp = /(?:Z|[+-]\d{2}:?\d{2})$/i.test(iso) ? iso : `${iso}Z`
  const date = new Date(timestamp)
  return Number.isNaN(date.getTime()) ? null : date
}

function formatPersistedTimestamp(value: string | null | undefined): string | null {
  const date = parsePersistedTimestamp(value)
  return date ? new Intl.DateTimeFormat(undefined, { dateStyle: 'short', timeStyle: 'medium' }).format(date) : null
}

function formatRunDuration(start: string | null | undefined, finish: string | null | undefined): string | null {
  const startedAt = parsePersistedTimestamp(start)
  const finishedAt = parsePersistedTimestamp(finish)
  if (!startedAt || !finishedAt) return null
  const seconds = Math.max(0, Math.round((finishedAt.getTime() - startedAt.getTime()) / 1000))
  return seconds < 60
    ? `${seconds}s`
    : `${Math.floor(seconds / 60)}m ${seconds % 60}s`
}

const nodeTitle = (nodes: HeimNode[], id: string) => {
  const node = nodes.find((item) => item.id === id && item.type === 'component')
  return node?.type === 'component' ? node.data.title : null
}

function TicketRunReviewPanel({
  ticket,
  review,
  loading,
  error,
  decisionNote,
  decisionSaving,
  decisionError,
  onDecisionNoteChange,
  onSaveDecision,
  onToggle,
}: {
  ticket: Ticket
  review: TicketRunReview | null
  loading: boolean
  error: string | null
  decisionNote: string
  decisionSaving: boolean
  decisionError: string | null
  onDecisionNoteChange: (note: string) => void
  onSaveDecision: (decision: TicketReviewDecisionKind, note: string) => void
  onToggle: () => void
}) {
  const isOpen = review?.runId === ticket.latestRunId
  const runActive = ticketIsRunning(ticket)
  const report = isOpen ? review.report : null
  const criteria = isOpen ? review.completionCriteria : ticket.completionCriteria
  const snapshots = isOpen ? review.evidence.filter((item) => item.kind === 'workspace_snapshot') : []
  const commands = isOpen ? review.evidence.filter((item) => item.kind === 'command_execution') : []

  return <div className="run-review">
    <button className="run-review-toggle" type="button" onClick={onToggle} disabled={loading || runActive || !ticket.latestRunId} aria-expanded={isOpen}>
      {loading ? 'Loading run details…' : runActive ? 'Run in progress' : isOpen ? 'Hide run details' : 'Review run details'}
    </button>
    {error && <p className="run-review-error" role="alert">Could not load run details: {error}</p>}
    {isOpen && <div className="run-review-content">
      <p className="run-review-note">Agent claims do not change status by themselves. Your decision below closes or reopens this ticket; map nodes are not changed.</p>
      {!report
        ? <p className="run-review-empty">No valid Agent report was saved for this run.</p>
        : <section className="run-review-section">
          <h4>Agent report · {report.outcome}</h4>
          <p>{report.summary}</p>
          <ul className="run-review-list">
            {report.criteria.map((criterion) => <li key={criterion.index}>
              <strong>{criterion.status === 'met' ? 'Reported met' : criterion.status === 'not_met' ? 'Not met' : 'Not run'}</strong>
              <span>{criteria[criterion.index] ?? `Criterion ${criterion.index + 1}`}</span>
              {criterion.evidence.length > 0 && <small>{criterion.evidence.join(' · ')}</small>}
            </li>)}
          </ul>
          {report.changes.length > 0 && <><h5>Agent-reported changes</h5><ul className="run-review-list">{report.changes.map((change) => <li key={`${change.path}-${change.summary}`}><code>{change.path}</code><span>{change.summary}</span></li>)}</ul></>}
          {report.checks.length > 0 && <><h5>Agent-reported checks</h5><ul className="run-review-list">{report.checks.map((check, index) => <li key={`${check.command}-${index}`}><strong>{check.status}</strong><code>{check.command}</code><span>{check.summary}</span></li>)}</ul></>}
          {report.blockers.length > 0 && <><h5>Blockers</h5><ul className="run-review-list">{report.blockers.map((blocker) => <li key={blocker}><span>{blocker}</span></li>)}</ul></>}
        </section>}
      <section className="run-review-section">
        <h4>Heim-observed evidence</h4>
        {snapshots.length === 0 && commands.length === 0 && <p className="run-review-empty">No execution evidence was captured.</p>}
        {snapshots.map((snapshot) => snapshot.evidence.available === false
          ? <p className="run-review-empty" key={snapshot.id}>Workspace comparison unavailable: {snapshot.evidence.error ?? 'unknown error'}</p>
          : <div key={snapshot.id}><h5>Changed files</h5>{snapshot.evidence.changedFiles?.length
            ? <ul className="run-review-list">{snapshot.evidence.changedFiles.map((path) => <li key={path}><code>{path}</code></li>)}</ul>
            : <p className="run-review-empty">No changed files detected in the workspace snapshot.</p>}</div>)}
        {commands.length > 0 && <><h5>Executed commands</h5><ul className="run-review-list">{commands.map((command) => <li key={command.id}>
          <code>{command.evidence.command ?? 'Command unavailable'}</code>
          <span>{command.evidence.status ?? 'Unknown status'}{command.evidence.exitCode != null ? ` · exit ${command.evidence.exitCode}` : ''}</span>
        </li>)}</ul></>}
      </section>
      <section className="run-review-section run-review-decision">
        <h4>User review decision</h4>
        {ticket.reviewDecision && <p className="run-review-note"><strong>{reviewDecisionLabels[ticket.reviewDecision]}</strong>{ticket.reviewedAt ? ` · ${formatPersistedTimestamp(ticket.reviewedAt) ?? ticket.reviewedAt}` : ''}{ticket.reviewNote ? ` · ${ticket.reviewNote}` : ''}</p>}
        <label>Review note or rework guidance
          <textarea rows={2} maxLength={2000} value={decisionNote} onChange={(event) => onDecisionNoteChange(event.target.value)} disabled={decisionSaving || runActive} />
        </label>
        {decisionError && <p className="run-review-error" role="alert">Could not save review decision: {decisionError}</p>}
        <div className="run-review-decisions">
          <button className="button button-primary" type="button" onClick={() => onSaveDecision('approved_completion', decisionNote)} disabled={decisionSaving || runActive}>{decisionSaving ? 'Saving…' : 'Approve · close ticket'}</button>
          <button className="button button-outline" type="button" onClick={() => onSaveDecision('rework_requested', decisionNote)} disabled={decisionSaving || runActive}>Request rework</button>
          <button className="button button-outline" type="button" onClick={() => onSaveDecision('blocked', decisionNote)} disabled={decisionSaving || runActive}>Keep blocked</button>
          <button className="button button-outline" type="button" onClick={() => onSaveDecision('incomplete', decisionNote)} disabled={decisionSaving || runActive}>Keep incomplete</button>
        </div>
      </section>
    </div>}
  </div>
}

function FeatureEditor({ initial, busy, onSubmit, onCancel }: {
  initial: { title: string; description: string }
  busy: boolean
  onSubmit: (title: string, description: string) => void
  onCancel: () => void
}) {
  const [title, setTitle] = useState(initial.title)
  const [description, setDescription] = useState(initial.description)
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (title.trim()) onSubmit(title.trim(), description.trim())
  }
  return <form className="ticket-editor" onSubmit={submit}>
    <label>Feature name<input value={title} maxLength={200} required disabled={busy} onChange={(event) => setTitle(event.target.value)} placeholder="e.g. Reading log" /></label>
    <label>What it should do<textarea rows={3} maxLength={4000} value={description} disabled={busy} onChange={(event) => setDescription(event.target.value)} placeholder="Describe the outcome; the AI uses this to split the feature into tickets." /></label>
    <div className="ticket-editor-actions">
      <button className="button button-primary" type="submit" disabled={busy || !title.trim()}>Save feature</button>
      <button className="button button-outline" type="button" onClick={onCancel} disabled={busy}>Cancel</button>
    </div>
  </form>
}

function TicketEditor({ initial, features, nodes, busy, onSubmit, onCancel }: {
  initial: TicketInput
  features: Feature[]
  nodes: HeimNode[]
  busy: boolean
  onSubmit: (input: TicketInput) => void
  onCancel: () => void
}) {
  const [title, setTitle] = useState(initial.title)
  const [goal, setGoal] = useState(initial.goal)
  const [criteria, setCriteria] = useState(initial.completionCriteria.join('\n'))
  const [featureId, setFeatureId] = useState(initial.featureId)
  const [nodeIds, setNodeIds] = useState(initial.nodeIds)
  const components = nodes.filter((node) => node.type === 'component')
  const removedNodeIds = nodeIds.filter((id) => !components.some((node) => node.id === id))
  const completionCriteria = criteria.split(/\r?\n/).map((item) => item.trim()).filter(Boolean)
  const valid = !!title.trim() && !!goal.trim() && completionCriteria.length > 0 && completionCriteria.length <= 20 && completionCriteria.every((item) => item.length <= 1000)
  const toggleNode = (id: string) => setNodeIds((current) => current.includes(id) ? current.filter((item) => item !== id) : [...current, id])
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (valid) onSubmit({ featureId, title: title.trim(), goal: goal.trim(), completionCriteria, nodeIds })
  }
  return <form className="ticket-editor" onSubmit={submit}>
    <label>Ticket title<input value={title} maxLength={200} required disabled={busy} onChange={(event) => setTitle(event.target.value)} /></label>
    <label>Goal — what to build in this session<textarea rows={3} maxLength={4000} value={goal} required disabled={busy} onChange={(event) => setGoal(event.target.value)} /></label>
    <label>Completion criteria (one per line)<textarea rows={3} value={criteria} disabled={busy} onChange={(event) => setCriteria(event.target.value)} /></label>
    {completionCriteria.length > 20 && <p className="ticket-error">A ticket can have at most 20 completion criteria.</p>}
    <label>Feature
      <select value={featureId ?? ''} disabled={busy} onChange={(event) => setFeatureId(event.target.value ? Number(event.target.value) : null)}>
        <option value="">No feature</option>
        {features.map((feature) => <option key={feature.id} value={feature.id}>{feature.title}</option>)}
      </select>
    </label>
    <fieldset className="ticket-node-picker" disabled={busy}>
      <legend>Map nodes this ticket touches</legend>
      {components.length === 0 && <p className="muted-copy">The map has no nodes yet.</p>}
      {components.map((node) => <label key={node.id} className="ticket-node-option">
        <input type="checkbox" checked={nodeIds.includes(node.id)} onChange={() => toggleNode(node.id)} />
        {node.type === 'component' ? node.data.title : node.id}
      </label>)}
      {removedNodeIds.map((id) => <label key={id} className="ticket-node-option is-removed">
        <input type="checkbox" checked onChange={() => toggleNode(id)} /> Removed from map · {id}
      </label>)}
    </fieldset>
    <div className="ticket-editor-actions">
      <button className="button button-primary" type="submit" disabled={busy || !valid}>Save ticket</button>
      <button className="button button-outline" type="button" onClick={onCancel} disabled={busy}>Cancel</button>
    </div>
  </form>
}

export type TicketBoardActions = {
  selectNode: (nodeId: string) => void
  expandTicket: (ticketId: number | null) => void
  createFeature: (title: string, description: string) => Promise<boolean>
  updateFeature: (featureId: number, title: string, description: string) => Promise<boolean>
  deleteFeature: (feature: Feature) => void
  moveFeature: (featureId: number, direction: 'up' | 'down') => void
  planFeature: (featureId: number) => void
  createTicket: (input: TicketInput) => Promise<boolean>
  updateTicket: (ticketId: number, input: TicketInput) => Promise<boolean>
  deleteTicket: (ticket: Ticket) => void
  moveTicket: (ticketId: number, direction: 'up' | 'down') => void
  setTicketStatus: (ticketId: number, status: 'todo' | 'done') => void
  startTicket: (ticketId: number) => void
  toggleRunReview: (runId: number) => void
  reviewNoteChange: (runId: number, note: string) => void
  saveReview: (ticket: Ticket, decision: TicketReviewDecisionKind, note: string) => void
}

export function TicketBoard({
  board,
  nodes,
  busy,
  developBlockedReason,
  planBlockedReason,
  planningFeatureId,
  expandedTicketId,
  newTicketRequest,
  review,
  reviewLoading,
  reviewError,
  reviewSavingRunId,
  reviewErrors,
  reviewNotes,
  actions,
}: {
  board: TicketBoardData
  nodes: HeimNode[]
  busy: boolean
  developBlockedReason: string | null
  planBlockedReason: string | null
  planningFeatureId: number | null
  expandedTicketId: number | null
  newTicketRequest: { nodeIds: string[]; nonce: number } | null
  review: TicketRunReview | null
  reviewLoading: number | null
  reviewError: { runId: number; message: string } | null
  reviewSavingRunId: number | null
  reviewErrors: Record<number, string>
  reviewNotes: Record<number, string>
  actions: TicketBoardActions
}) {
  const [editingFeature, setEditingFeature] = useState<number | 'new' | null>(null)
  const [editingTicket, setEditingTicket] = useState<{ id: number | null; initial: TicketInput } | null>(null)

  useEffect(() => {
    if (!newTicketRequest) return
    setEditingTicket({ id: null, initial: { featureId: null, title: '', goal: '', completionCriteria: [], nodeIds: newTicketRequest.nodeIds } })
  }, [newTicketRequest])

  const nextTicket = board.tickets.find((ticket) => ticket.id === board.nextTicketId) ?? null
  const groups: { feature: Feature | null; tickets: Ticket[] }[] = [
    ...board.features.map((feature) => ({ feature, tickets: board.tickets.filter((ticket) => ticket.featureId === feature.id) })),
    { feature: null, tickets: board.tickets.filter((ticket) => ticket.featureId === null) },
  ]

  const saveTicket = async (input: TicketInput) => {
    if (!editingTicket) return
    const saved = editingTicket.id === null ? await actions.createTicket(input) : await actions.updateTicket(editingTicket.id, input)
    if (saved) setEditingTicket(null)
  }

  const ticketEditor = (key: string) => editingTicket && <TicketEditor
    key={key}
    initial={editingTicket.initial}
    features={board.features}
    nodes={nodes}
    busy={busy}
    onSubmit={(input) => { void saveTicket(input) }}
    onCancel={() => setEditingTicket(null)}
  />

  const renderTicket = (ticket: Ticket, index: number, siblings: Ticket[]) => {
    const expanded = expandedTicketId === ticket.id
    const running = ticketIsRunning(ticket)
    const isNext = ticket.id === board.nextTicketId
    if (editingTicket?.id === ticket.id) return ticketEditor(`edit-${ticket.id}`)
    const runStartedAt = formatPersistedTimestamp(ticket.latestRunStartedAt)
    const runDuration = formatRunDuration(ticket.latestRunStartedAt, ticket.latestRunFinishedAt)
    return <article key={ticket.id} className={`ticket-card is-${ticket.status}${isNext ? ' is-next' : ''}${expanded ? ' is-expanded' : ''}`}>
      <div className="ticket-card-row">
        <button type="button" className="ticket-card-title" onClick={() => actions.expandTicket(expanded ? null : ticket.id)} aria-expanded={expanded}>
          {isNext && <span className="ticket-next-badge">NEXT</span>}
          <strong>{ticket.title}</strong>
          <span className={`ticket-status-pill is-${ticket.status}`}>{ticketStatusText(ticket)}</span>
        </button>
        <div className="ticket-card-actions">
          {ticket.status !== 'done' && <button className="button button-development ticket-develop" type="button" onClick={() => actions.startTicket(ticket.id)} disabled={busy || running || !!developBlockedReason} title={developBlockedReason ?? `Start an Agent session for “${ticket.title}”`}>
            <Code2 size={14} /> Develop
          </button>}
          <button className="icon-button compact-icon-button" type="button" onClick={() => actions.moveTicket(ticket.id, 'up')} disabled={busy || index === 0} aria-label={`Move ${ticket.title} up`}><ArrowUp size={14} /></button>
          <button className="icon-button compact-icon-button" type="button" onClick={() => actions.moveTicket(ticket.id, 'down')} disabled={busy || index === siblings.length - 1} aria-label={`Move ${ticket.title} down`}><ArrowDown size={14} /></button>
        </div>
      </div>
      <div className="ticket-node-chips">
        {ticket.nodeIds.length === 0 && <span className="ticket-node-chip is-empty">No map node linked</span>}
        {ticket.nodeIds.map((id) => {
          const title = nodeTitle(nodes, id)
          return title
            ? <button type="button" key={id} className="ticket-node-chip" onClick={() => actions.selectNode(id)} title="Show on map">{title}</button>
            : <span key={id} className="ticket-node-chip is-removed" title="This node was removed from the map">Removed · {id}</span>
        })}
        {ticket.source === 'agent' && <span className="ticket-source"><Sparkles size={11} /> AI</span>}
      </div>
      {expanded && <div className="ticket-card-body">
        <p>{ticket.goal}</p>
        <ul>{ticket.completionCriteria.map((criterion) => <li key={criterion}>{criterion}</li>)}</ul>
        {ticket.latestRunId && <small className="ticket-run-line">Run #{ticket.latestRunId}{ticket.latestRunState ? ` · ${runStateLabels[ticket.latestRunState]}` : ''}{runStartedAt ? ` · started ${runStartedAt}` : ''}{runDuration ? ` · ${runDuration}` : ''}{ticket.reportOutcome ? ` · Agent reported ${ticket.reportOutcome}` : ''}</small>}
        {ticket.latestRunDetail && <small className="ticket-error" role="alert">{ticket.latestRunDetail}</small>}
        <div className="ticket-card-tools">
          <button className="text-action" type="button" onClick={() => setEditingTicket({ id: ticket.id, initial: { featureId: ticket.featureId, title: ticket.title, goal: ticket.goal, completionCriteria: ticket.completionCriteria, nodeIds: ticket.nodeIds } })} disabled={busy || running}><Pencil size={13} /> Edit</button>
          {ticket.status === 'done'
            ? <button className="text-action" type="button" onClick={() => actions.setTicketStatus(ticket.id, 'todo')} disabled={busy || running}><RotateCcw size={13} /> Reopen</button>
            : <button className="text-action" type="button" onClick={() => actions.setTicketStatus(ticket.id, 'done')} disabled={busy || running}><Check size={13} /> Mark done</button>}
          <button className="text-action is-danger" type="button" onClick={() => actions.deleteTicket(ticket)} disabled={busy || running}><Trash2 size={13} /> Delete</button>
        </div>
        {ticket.latestRunId && <TicketRunReviewPanel
          ticket={ticket}
          review={review?.runId === ticket.latestRunId ? review : null}
          loading={reviewLoading === ticket.latestRunId}
          error={reviewError?.runId === ticket.latestRunId ? reviewError.message : null}
          decisionNote={reviewNotes[ticket.latestRunId] ?? ticket.reviewNote ?? ''}
          decisionSaving={reviewSavingRunId === ticket.latestRunId}
          decisionError={reviewErrors[ticket.latestRunId] ?? null}
          onDecisionNoteChange={(note) => actions.reviewNoteChange(ticket.latestRunId!, note)}
          onSaveDecision={(decision, note) => actions.saveReview(ticket, decision, note)}
          onToggle={() => actions.toggleRunReview(ticket.latestRunId!)}
        />}
      </div>}
    </article>
  }

  return <div className="workspace-content ticket-board">
    <div className="ticket-board-header">
      <div className="ticket-board-next">
        <ListChecks size={16} />
        {nextTicket
          ? <span>Next: <button type="button" className="ticket-next-link" onClick={() => actions.expandTicket(nextTicket.id)}>{nextTicket.title}</button></span>
          : <span>{board.tickets.length ? 'No open tickets — every ticket is done or in progress.' : 'No tickets yet. Add a feature and split it into tickets, or add a ticket directly.'}</span>}
      </div>
      <div className="ticket-board-actions">
        <button className="button button-outline" type="button" onClick={() => setEditingFeature('new')} disabled={busy}><Plus size={14} /> Feature</button>
        <button className="button button-outline" type="button" onClick={() => setEditingTicket({ id: null, initial: { featureId: null, title: '', goal: '', completionCriteria: [], nodeIds: [] } })} disabled={busy}><Plus size={14} /> Ticket</button>
      </div>
    </div>
    {editingFeature === 'new' && <FeatureEditor initial={{ title: '', description: '' }} busy={busy} onCancel={() => setEditingFeature(null)} onSubmit={(title, description) => { void actions.createFeature(title, description).then((saved) => { if (saved) setEditingFeature(null) }) }} />}
    {editingTicket?.id === null && ticketEditor('new')}
    {groups.map(({ feature, tickets }, groupIndex) => {
      if (!feature && tickets.length === 0) return null
      const planning = feature && planningFeatureId === feature.id
      return <section key={feature?.id ?? 'unassigned'} className={`feature-group${feature?.completed ? ' is-completed' : ''}`}>
        {feature && editingFeature === feature.id
          ? <FeatureEditor initial={feature} busy={busy} onCancel={() => setEditingFeature(null)} onSubmit={(title, description) => { void actions.updateFeature(feature.id, title, description).then((saved) => { if (saved) setEditingFeature(null) }) }} />
          : <header className="feature-header">
            <div>
              <strong>{feature ? feature.title : 'Tickets without a feature'}</strong>
              {feature && <span className="feature-progress">{feature.completed ? <><Check size={12} /> Complete</> : `${feature.doneCount}/${feature.ticketCount} done`}</span>}
              {feature?.description && <p>{feature.description}</p>}
            </div>
            {feature && <div className="feature-actions">
              <button className="button button-outline" type="button" onClick={() => actions.planFeature(feature.id)} disabled={busy || !!planBlockedReason || planningFeatureId !== null} title={planBlockedReason ?? 'Ask the AI to split this feature into tickets'}>
                {planning ? <LoaderCircle className="send-spinner" size={14} /> : <Sparkles size={14} />} {planning ? 'Planning…' : 'Split into tickets'}
              </button>
              <button className="icon-button compact-icon-button" type="button" onClick={() => setEditingFeature(feature.id)} disabled={busy} aria-label={`Edit ${feature.title}`}><Pencil size={14} /></button>
              <button className="icon-button compact-icon-button" type="button" onClick={() => actions.moveFeature(feature.id, 'up')} disabled={busy || groupIndex === 0} aria-label={`Move ${feature.title} up`}><ArrowUp size={14} /></button>
              <button className="icon-button compact-icon-button" type="button" onClick={() => actions.moveFeature(feature.id, 'down')} disabled={busy || groupIndex === board.features.length - 1} aria-label={`Move ${feature.title} down`}><ArrowDown size={14} /></button>
              <button className="icon-button compact-icon-button" type="button" onClick={() => actions.deleteFeature(feature)} disabled={busy} aria-label={`Delete ${feature.title}`}><Trash2 size={14} /></button>
            </div>}
          </header>}
        {tickets.length === 0 && feature && <p className="muted-copy">No tickets yet. Split it with AI or add a ticket.</p>}
        {tickets.map((ticket, index) => renderTicket(ticket, index, tickets))}
      </section>
    })}
  </div>
}

/** Tickets that reference one map node: the node's development history. */
export function NodeTicketList({ tickets, onOpen, onCreate, canCreate }: {
  tickets: Ticket[]
  onOpen: (ticketId: number) => void
  onCreate: () => void
  canCreate: boolean
}) {
  return <div className="node-ticket-list">
    {tickets.length === 0
      ? <div className="detail-empty"><ListChecks size={20} /><strong>No tickets touch this node yet</strong><p>Tickets are units of work; a node only shows which tickets touched it.</p></div>
      : tickets.map((ticket) => <button type="button" key={ticket.id} className="node-ticket-row" onClick={() => onOpen(ticket.id)}>
        <span className={`ticket-status-pill is-${ticket.status}`}>{ticketStatusText(ticket)}</span>
        <span>{ticket.title}</span>
      </button>)}
    <button className="button button-outline" type="button" onClick={onCreate} disabled={!canCreate}><Plus size={14} /> New ticket for this node</button>
  </div>
}
