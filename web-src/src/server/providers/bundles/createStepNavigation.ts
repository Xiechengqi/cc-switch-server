export type CreateStep = "family" | "supply" | "share";

export const CREATE_STEPS: readonly CreateStep[] = [
  "family",
  "supply",
  "share",
];

function stepIndex(step: CreateStep): number {
  return CREATE_STEPS.indexOf(step);
}

export function canVisitCreateStep(
  step: CreateStep,
  highestReached: CreateStep,
): boolean {
  return stepIndex(step) <= stepIndex(highestReached);
}

export function nextCreateStep(step: CreateStep): CreateStep | null {
  return CREATE_STEPS[stepIndex(step) + 1] ?? null;
}

export function previousCreateStep(step: CreateStep): CreateStep | null {
  return CREATE_STEPS[stepIndex(step) - 1] ?? null;
}

export function unlockCreateStep(
  highestReached: CreateStep,
  step: CreateStep,
): CreateStep {
  return stepIndex(step) > stepIndex(highestReached) ? step : highestReached;
}
