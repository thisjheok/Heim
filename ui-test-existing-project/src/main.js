const taskInputs = document.querySelectorAll('input[type="checkbox"]')
const progress = document.querySelector('footer strong')

function updateProgress() {
  const done = [...taskInputs].filter((input) => input.checked).length
  progress.textContent = `${done} / ${taskInputs.length}`
}

taskInputs.forEach((input) => input.addEventListener('change', updateProgress))
