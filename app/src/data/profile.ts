// Project profile (docs/decisions/0002): output form and stack, decided in the map draft and confirmed on approval.
export type StackArea = { area: string; items: string[] }
export type ProfileSource = 'user_specified' | 'detected' | 'ai_proposed'
export type ProjectProfile = { outputForm: string; stack: StackArea[]; constraints: string[]; source: ProfileSource; rationale: string }

export const draftProfileSourceLabels: Record<ProfileSource, string> = {
  user_specified: 'Specified in your request',
  detected: 'Detected from existing files',
  ai_proposed: 'Proposed by AI — ask in chat to change it before approving',
}

export const confirmedProfileSourceLabels: Record<ProfileSource, string> = {
  user_specified: 'Specified in your request',
  detected: 'Detected from existing files',
  ai_proposed: 'Proposed by AI, confirmed with the map',
}

export const stackAreaSummary = (profile: ProjectProfile) =>
  profile.stack.length === 1 ? profile.stack[0].items.join(' + ') : profile.stack.map((area) => area.area).join(' · ')
