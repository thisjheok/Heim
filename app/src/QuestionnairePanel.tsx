import { AlertCircle } from 'lucide-react'
import type { DecisionQuestion } from './data/projectMap'

type QuestionnairePanelProps = {
  questions: DecisionQuestion[]
  answers: Record<string, string>
  disabled?: boolean
  onAnswer: (prompt: string, answer: string) => void
}

export function QuestionnairePanel({ questions, answers, disabled = false, onAnswer }: QuestionnairePanelProps) {
  if (questions.length === 0) return null

  const unansweredCount = questions.filter((question) => !(answers[question.prompt] ?? '').trim()).length

  return (
    <section className="draft-question-panel detail-question-panel" aria-label="Open questions">
      <h3><AlertCircle size={16} /> Open questions</h3>
      {questions.map((question) => {
        const answer = answers[question.prompt] ?? ''
        const isCustomAnswer = answer !== '' && !question.options.some((option) => option.label === answer)
        return (
          <div className="question-answer" key={question.prompt}>
            <span>{question.prompt}</span>
            <div className="question-options" role="group" aria-label={question.prompt}>
              {question.options.map((option, index) => (
                <button
                  className={`question-option${answer === option.label ? ' is-selected' : ''}`}
                  type="button"
                  key={option.label}
                  aria-pressed={answer === option.label}
                  disabled={disabled}
                  onClick={() => onAnswer(question.prompt, option.label)}
                >
                  <span>{index + 1}. {option.label}</span>
                  {option.recommended && <small>recommended</small>}
                </button>
              ))}
              <button
                className={`question-option question-custom-option${isCustomAnswer ? ' is-selected' : ''}`}
                type="button"
                aria-pressed={isCustomAnswer}
                disabled={disabled}
                onClick={() => onAnswer(question.prompt, isCustomAnswer ? '' : ' ')}
              >
                <span>직접 입력</span>
              </button>
            </div>
            {isCustomAnswer && (
              <textarea
                rows={2}
                aria-label={`${question.prompt} 직접 답변`}
                placeholder="직접 답변을 입력하세요…"
                value={answer.trim()}
                disabled={disabled}
                onChange={(event) => onAnswer(question.prompt, event.target.value)}
              />
            )}
          </div>
        )
      })}
      <small>{unansweredCount > 0
        ? 'Answer all questions, then save the map before development.'
        : 'All questions answered. Save the map to confirm these decisions.'}</small>
    </section>
  )
}
