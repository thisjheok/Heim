import { Layers } from 'lucide-react'
import { confirmedProfileSourceLabels, type ProjectProfile, type StackArea } from './data/profile'

/** One row per area: the area name, then its technologies as tags. */
export function StackAreaList({ stack }: { stack: StackArea[] }) {
  return (
    <dl className="stack-area-list">
      {stack.map((area) => (
        <div className="stack-area-row" key={area.area}>
          <dt>{area.area}</dt>
          <dd>
            <ul className="stack-tags">
              {area.items.map((item) => <li key={item}>{item}</li>)}
            </ul>
          </dd>
        </div>
      ))}
    </dl>
  )
}

/** Bottom workspace "Stack" tab: the confirmed profile, or how it gets set. */
export function ProjectStackPanel({ profile }: { profile: ProjectProfile | null }) {
  return (
    <div className="workspace-content stack-content">
      <div className="workspace-content-heading"><Layers size={17} /><strong>Project stack</strong></div>
      {profile
        ? <>
          <section className="stack-section" aria-label="Output form">
            <span className="draft-summary-label">Output</span>
            <strong className="stack-output">{profile.outputForm}</strong>
            <span className={`draft-profile-source is-${profile.source}`}>{confirmedProfileSourceLabels[profile.source]}</span>
          </section>
          <section className="stack-section" aria-label="Stack by area">
            <span className="draft-summary-label">Stack by area</span>
            <StackAreaList stack={profile.stack} />
          </section>
          {profile.constraints.length > 0 && <section className="stack-section" aria-label="Constraints">
            <span className="draft-summary-label">Constraints</span>
            <ul className="draft-summary-list">
              {profile.constraints.map((constraint) => <li key={constraint}>{constraint}</li>)}
            </ul>
          </section>}
          {profile.rationale && <details className="draft-summary-details">
            <summary>Why this stack</summary>
            <p>{profile.rationale}</p>
          </details>}
          <p className="draft-summary-note">Every ticket is developed with this stack. To change it, ask the Agent in chat for a map draft revision and approve it.</p>
        </>
        : <p className="draft-summary-note">No output form or stack is confirmed yet. It is set when you approve a map design draft; until then the Agent follows the existing code, and reports a blocker in an empty folder.</p>}
    </div>
  )
}
