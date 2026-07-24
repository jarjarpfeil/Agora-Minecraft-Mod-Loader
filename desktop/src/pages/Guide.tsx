import { useMemo, useRef, useState, type ComponentType } from 'react';
import {
  ArrowLeft,
  ArrowRight,
  BookOpen,
  Check,
  CheckCircle2,
  ChevronRight,
  Circle,
  Compass,
  Gamepad2,
  GraduationCap,
  Info,
  Lightbulb,
  Network,
  Paintbrush,
  RotateCcw,
  Search,
  ShieldCheck,
  SlidersHorizontal,
  TriangleAlert,
  X,
} from 'lucide-react';
import {
  GUIDE_CATEGORIES,
  GUIDE_TOPICS,
  type GuideCallout,
  type GuideLevel,
  type GuidePage,
  type GuideTopic,
} from '../data/guideContent';
import type { Tab } from '../lib/useDestination';

const GUIDE_STATE_KEY = 'agora-guide-state';
const GUIDE_PROGRESS_KEY = 'agora-guide-progress';

interface StoredGuideState {
  topicId: string;
  level: GuideLevel;
}

interface TopicDestination {
  label: string;
  tab: Tab;
}

const TOPIC_DESTINATIONS: Record<string, TopicDestination> = {
  'getting-started': { label: 'Open Home', tab: 'home' },
  'modding-foundations': { label: 'Explore Browse', tab: 'browse' },
  'home-navigation': { label: 'Open Home', tab: 'home' },
  instances: { label: 'Open My Instances', tab: 'instances' },
  'browse-registry': { label: 'Open Browse', tab: 'browse' },
  'install-update': { label: 'Find content to install', tab: 'browse' },
  'content-management': { label: 'Open My Instances', tab: 'instances' },
  launching: { label: 'Open My Instances', tab: 'instances' },
  'crash-recovery': { label: 'Open My Instances', tab: 'instances' },
  'snapshots-loadouts': { label: 'Open My Instances', tab: 'instances' },
  'packs-sharing': { label: 'Open My Instances', tab: 'instances' },
  'java-performance': { label: 'Open Settings', tab: 'settings' },
  'settings-appearance': { label: 'Open Settings', tab: 'settings' },
  'accounts-services': { label: 'Open Settings', tab: 'settings' },
  'privacy-offline': { label: 'Open Settings', tab: 'settings' },
  governance: { label: 'Open Governance', tab: 'governance' },
  'ai-assistant': { label: 'Configure AI Assistant', tab: 'settings' },
  'mcp-automation': { label: 'Configure MCP', tab: 'settings' },
};

const CATEGORY_ICONS: Record<GuideTopic['category'], ComponentType<{ className?: string }>> = {
  Start: Compass,
  Play: Gamepad2,
  Manage: SlidersHorizontal,
  Recover: ShieldCheck,
  Customize: Paintbrush,
  Connect: Network,
};

const LEVEL_COPY: Record<GuideLevel, { label: string; description: string }> = {
  basic: {
    label: 'Basic guide',
    description: 'Clear concepts and step-by-step workflows. No modding experience required.',
  },
  advanced: {
    label: 'Advanced guide',
    description: 'Deeper controls, tradeoffs, diagnostics, security, and repeatable workflows.',
  },
};

function loadGuideState(): StoredGuideState {
  const fallback: StoredGuideState = { topicId: GUIDE_TOPICS[0].id, level: 'basic' };
  try {
    const value = JSON.parse(localStorage.getItem(GUIDE_STATE_KEY) ?? 'null') as Partial<StoredGuideState> | null;
    if (!value || typeof value.topicId !== 'string' || !GUIDE_TOPICS.some((topic) => topic.id === value.topicId)) return fallback;
    if (value.level !== 'basic' && value.level !== 'advanced') return fallback;
    return { topicId: value.topicId, level: value.level };
  } catch {
    return fallback;
  }
}

function loadProgress(): Set<string> {
  try {
    const value = JSON.parse(localStorage.getItem(GUIDE_PROGRESS_KEY) ?? '[]') as unknown;
    if (!Array.isArray(value)) return new Set();
    return new Set(value.filter((entry): entry is string => typeof entry === 'string'));
  } catch {
    return new Set();
  }
}

function pageKey(topicId: string, level: GuideLevel) {
  return `${topicId}:${level}`;
}

function pageText(page: GuidePage) {
  return [
    page.summary,
    ...page.outcomes,
    ...page.sections.flatMap((section) => [
      section.title,
      section.body,
      ...(section.steps ?? []),
      ...(section.bullets ?? []),
      section.callout?.title ?? '',
      section.callout?.text ?? '',
    ]),
  ].join(' ');
}

export function Guide({ onNavigateTab }: { onNavigateTab: (tab: Tab) => void }) {
  const initialState = useMemo(loadGuideState, []);
  const [selectedTopicId, setSelectedTopicId] = useState(initialState.topicId);
  const [level, setLevel] = useState<GuideLevel>(initialState.level);
  const [query, setQuery] = useState('');
  const [completedPages, setCompletedPages] = useState<Set<string>>(loadProgress);
  const articleRef = useRef<HTMLElement>(null);

  const selectedTopic = GUIDE_TOPICS.find((topic) => topic.id === selectedTopicId) ?? GUIDE_TOPICS[0];
  const selectedPage = selectedTopic[level];
  const selectedPageKey = pageKey(selectedTopic.id, level);
  const selectedPageComplete = completedPages.has(selectedPageKey);
  const totalPages = GUIDE_TOPICS.length * 2;
  const completedCount = [...completedPages].filter((key) =>
    GUIDE_TOPICS.some((topic) => key === pageKey(topic.id, 'basic') || key === pageKey(topic.id, 'advanced')),
  ).length;
  const progressPercent = Math.round((completedCount / totalPages) * 100);

  const normalizedQuery = query.trim().toLowerCase();
  const filteredTopics = useMemo(() => {
    if (!normalizedQuery) return GUIDE_TOPICS;
    return GUIDE_TOPICS.filter((topic) => [
      topic.title,
      topic.description,
      topic.category,
      ...topic.keywords,
      pageText(topic.basic),
      pageText(topic.advanced),
    ].join(' ').toLowerCase().includes(normalizedQuery));
  }, [normalizedQuery]);

  const flatPages = useMemo(() => GUIDE_TOPICS.flatMap((topic) => [
    { topicId: topic.id, level: 'basic' as const },
    { topicId: topic.id, level: 'advanced' as const },
  ]), []);
  const currentPageIndex = flatPages.findIndex(
    (entry) => entry.topicId === selectedTopic.id && entry.level === level,
  );
  const previousPage = currentPageIndex > 0 ? flatPages[currentPageIndex - 1] : null;
  const nextPage = currentPageIndex < flatPages.length - 1 ? flatPages[currentPageIndex + 1] : null;

  const persistState = (topicId: string, nextLevel: GuideLevel) => {
    try {
      localStorage.setItem(GUIDE_STATE_KEY, JSON.stringify({ topicId, level: nextLevel }));
    } catch {
      // Guide navigation remains usable when local storage is unavailable.
    }
  };

  const selectPage = (topicId: string, nextLevel: GuideLevel = level) => {
    setSelectedTopicId(topicId);
    setLevel(nextLevel);
    persistState(topicId, nextLevel);
    requestAnimationFrame(() => articleRef.current?.scrollIntoView({ block: 'start' }));
  };

  const selectLevel = (nextLevel: GuideLevel) => {
    setLevel(nextLevel);
    persistState(selectedTopic.id, nextLevel);
    requestAnimationFrame(() => articleRef.current?.scrollIntoView({ block: 'start' }));
  };

  const toggleComplete = () => {
    setCompletedPages((current) => {
      const next = new Set(current);
      if (next.has(selectedPageKey)) {
        next.delete(selectedPageKey);
      } else {
        next.add(selectedPageKey);
      }
      try {
        localStorage.setItem(GUIDE_PROGRESS_KEY, JSON.stringify([...next]));
      } catch {
        // Progress remains available for the current session.
      }
      return next;
    });
  };

  const resetProgress = () => {
    setCompletedPages(new Set());
    try {
      localStorage.removeItem(GUIDE_PROGRESS_KEY);
    } catch {
      // Ignore unavailable local storage.
    }
  };

  const goToFlatPage = (target: { topicId: string; level: GuideLevel } | null) => {
    if (target) selectPage(target.topicId, target.level);
  };

  const destination = TOPIC_DESTINATIONS[selectedTopic.id];
  const currentTopicIndex = GUIDE_TOPICS.findIndex((topic) => topic.id === selectedTopic.id);

  return (
    <div className="mx-auto max-w-[1500px] space-y-6" data-testid="guide-page">
      <header className="relative overflow-hidden rounded-2xl border border-primary/25 bg-[linear-gradient(125deg,hsl(var(--midnight)),hsl(var(--aegean)))] p-6 text-white shadow-lg sm:p-8">
        <div className="absolute -right-16 -top-20 h-56 w-56 rounded-full border border-white/10" aria-hidden="true" />
        <div className="absolute -bottom-28 right-20 h-64 w-64 rounded-full border border-white/10" aria-hidden="true" />
        <div className="relative grid gap-6 lg:grid-cols-[minmax(0,1fr)_22rem] lg:items-end">
          <div className="max-w-3xl">
            <div className="mb-4 flex h-11 w-11 items-center justify-center rounded-xl bg-white/10 ring-1 ring-white/20">
              <BookOpen className="h-6 w-6" aria-hidden="true" />
            </div>
            <p className="text-xs font-bold uppercase tracking-[0.2em] text-white/65">Agora field guide</p>
            <h1 className="mt-2 text-3xl font-bold tracking-tight sm:text-4xl">Learn the launcher. Understand your modded game.</h1>
            <p className="mt-3 max-w-2xl text-sm leading-6 text-white/75 sm:text-base">
              {GUIDE_TOPICS.length} topics, each with a beginner-friendly walkthrough and a separate advanced guide.
              Start anywhere and move at your own pace.
            </p>
          </div>
          <div className="rounded-xl border border-white/15 bg-black/15 p-4 backdrop-blur-sm">
            <div className="flex items-center justify-between text-xs font-semibold">
              <span>Guide progress</span>
              <span>{completedCount} of {totalPages} pages</span>
            </div>
            <div
              role="progressbar"
              aria-label="Guide completion"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={progressPercent}
              className="mt-3 h-2 overflow-hidden rounded-full bg-white/15"
            >
              <div className="h-full rounded-full bg-[hsl(var(--civic-gold))] transition-[width]" style={{ width: `${progressPercent}%` }} />
            </div>
            <div className="mt-3 flex items-center justify-between gap-3 text-xs text-white/70">
              <span>{progressPercent}% complete</span>
              {completedCount > 0 && (
                <button onClick={resetProgress} className="inline-flex items-center gap-1 hover:text-white">
                  <RotateCcw className="h-3 w-3" aria-hidden="true" />
                  Reset
                </button>
              )}
            </div>
          </div>
        </div>
      </header>

      <section aria-labelledby="guide-start-heading" className="grid gap-3 md:grid-cols-3">
        <h2 id="guide-start-heading" className="sr-only">Choose a starting point</h2>
        <JourneyCard
          eyebrow="New to modding"
          title="Start with the essentials"
          description="Learn instances, loaders, versions, and your first safe launch."
          onClick={() => selectPage('getting-started', 'basic')}
        />
        <JourneyCard
          eyebrow="Know the basics"
          title="Build safer workflows"
          description="Use install plans, snapshots, loadouts, and Crash Doctor confidently."
          onClick={() => selectPage('install-update', 'basic')}
        />
        <JourneyCard
          eyebrow="Experienced modder"
          title="Go straight to advanced"
          description="Tune Java, lock reproduction state, audit privacy, and automate safely."
          onClick={() => selectPage('java-performance', 'advanced')}
        />
      </section>

      <div className="grid items-start gap-6 xl:grid-cols-[17rem_minmax(0,1fr)]">
        <aside className="space-y-4 xl:sticky xl:top-0" aria-label="Guide topics">
          <div className="rounded-xl border border-border bg-card p-3 shadow-sm">
            <label htmlFor="guide-search" className="sr-only">Search the guide</label>
            <div className="relative">
              <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" aria-hidden="true" />
              <input
                id="guide-search"
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder="Search every guide..."
                className="h-10 w-full rounded-lg border border-input bg-background pl-9 pr-9 text-sm outline-none focus:ring-2 focus:ring-ring"
              />
              {query && (
                <button
                  onClick={() => setQuery('')}
                  aria-label="Clear guide search"
                  className="absolute right-2 top-1/2 -translate-y-1/2 rounded p-1 text-muted-foreground hover:bg-accent hover:text-foreground"
                >
                  <X className="h-4 w-4" aria-hidden="true" />
                </button>
              )}
            </div>
            {normalizedQuery && (
              <p className="mt-2 px-1 text-xs text-muted-foreground">
                {filteredTopics.length} {filteredTopics.length === 1 ? 'topic' : 'topics'} found
              </p>
            )}
          </div>

          <nav className="max-h-[calc(100vh_-_9rem)] space-y-4 overflow-y-auto rounded-xl border border-border bg-card p-3 shadow-sm">
            {filteredTopics.length === 0 ? (
              <div className="px-2 py-6 text-center">
                <Search className="mx-auto h-5 w-5 text-muted-foreground" aria-hidden="true" />
                <p className="mt-2 text-sm font-medium">No matching guides</p>
                <p className="mt-1 text-xs text-muted-foreground">Try a feature name, action, or error concept.</p>
              </div>
            ) : (
              GUIDE_CATEGORIES.map((category) => {
                const categoryTopics = filteredTopics.filter((topic) => topic.category === category);
                if (categoryTopics.length === 0) return null;
                const CategoryIcon = CATEGORY_ICONS[category];
                return (
                  <div key={category}>
                    <div className="mb-1.5 flex items-center gap-2 px-2 text-[11px] font-bold uppercase tracking-[0.16em] text-muted-foreground">
                      <CategoryIcon className="h-3.5 w-3.5" aria-hidden="true" />
                      {category}
                    </div>
                    <div className="space-y-0.5">
                      {categoryTopics.map((topic) => {
                        const active = topic.id === selectedTopic.id;
                        const complete = completedPages.has(pageKey(topic.id, 'basic'))
                          && completedPages.has(pageKey(topic.id, 'advanced'));
                        return (
                          <button
                            key={topic.id}
                            onClick={() => selectPage(topic.id)}
                            aria-current={active ? 'page' : undefined}
                            className={[
                              'flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-left text-sm transition-colors',
                              active
                                ? 'bg-primary text-primary-foreground'
                                : 'text-muted-foreground hover:bg-accent hover:text-accent-foreground',
                            ].join(' ')}
                          >
                            {complete ? (
                              <CheckCircle2 className="h-4 w-4 shrink-0" aria-hidden="true" />
                            ) : (
                              <Circle className="h-4 w-4 shrink-0 opacity-50" aria-hidden="true" />
                            )}
                            <span className="min-w-0 flex-1 truncate">{topic.shortTitle}</span>
                            {active && <ChevronRight className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />}
                          </button>
                        );
                      })}
                    </div>
                  </div>
                );
              })
            )}
          </nav>
        </aside>

        <article ref={articleRef} className="min-w-0 overflow-hidden rounded-2xl border border-border bg-card shadow-sm">
          <div className="border-b border-border bg-muted/30 px-5 py-5 sm:px-7">
            <div className="flex flex-wrap items-center justify-between gap-3">
              <p className="text-xs font-bold uppercase tracking-[0.16em] text-primary">
                Topic {currentTopicIndex + 1} of {GUIDE_TOPICS.length} / {selectedTopic.category}
              </p>
              <button
                onClick={toggleComplete}
                className={[
                  'inline-flex items-center gap-2 rounded-lg border px-3 py-1.5 text-xs font-semibold transition-colors',
                  selectedPageComplete
                    ? 'border-primary bg-primary text-primary-foreground'
                    : 'border-border bg-card hover:bg-accent',
                ].join(' ')}
              >
                {selectedPageComplete ? <Check className="h-3.5 w-3.5" aria-hidden="true" /> : <Circle className="h-3.5 w-3.5" aria-hidden="true" />}
                {selectedPageComplete ? 'Completed' : 'Mark complete'}
              </button>
            </div>
            <h2 className="mt-2 text-2xl font-bold tracking-tight sm:text-3xl">{selectedTopic.title}</h2>
            <p className="mt-2 max-w-3xl text-sm leading-6 text-muted-foreground">{selectedTopic.description}</p>

            <div className="mt-5 grid gap-2 sm:grid-cols-2" role="group" aria-label="Experience level">
              {(['basic', 'advanced'] as const).map((option) => {
                const active = level === option;
                return (
                  <button
                    key={option}
                    onClick={() => selectLevel(option)}
                    aria-pressed={active}
                    className={[
                      'rounded-xl border p-3 text-left transition-colors',
                      active
                        ? 'border-primary bg-primary/10 ring-1 ring-primary/20'
                        : 'border-border bg-card hover:bg-accent',
                    ].join(' ')}
                  >
                    <span className="flex items-center gap-2 text-sm font-semibold">
                      {option === 'basic'
                        ? <BookOpen className="h-4 w-4 text-primary" aria-hidden="true" />
                        : <GraduationCap className="h-4 w-4 text-primary" aria-hidden="true" />}
                      {LEVEL_COPY[option].label}
                      {completedPages.has(pageKey(selectedTopic.id, option)) && (
                        <CheckCircle2 className="ml-auto h-4 w-4 text-primary" aria-hidden="true" />
                      )}
                    </span>
                    <span className="mt-1 block text-xs leading-5 text-muted-foreground">{LEVEL_COPY[option].description}</span>
                  </button>
                );
              })}
            </div>
          </div>

          <div className="space-y-8 px-5 py-6 sm:px-7 sm:py-8">
            <section className="grid gap-5 lg:grid-cols-[minmax(0,1fr)_18rem]">
              <div>
                <p className="text-xs font-bold uppercase tracking-[0.16em] text-muted-foreground">{LEVEL_COPY[level].label}</p>
                <p className="mt-2 text-base leading-7 text-foreground">{selectedPage.summary}</p>
              </div>
              <div className="rounded-xl border border-primary/20 bg-primary/5 p-4">
                <h3 className="text-sm font-semibold">After this guide, you can</h3>
                <ul className="mt-3 space-y-2.5">
                  {selectedPage.outcomes.map((outcome) => (
                    <li key={outcome} className="flex gap-2 text-xs leading-5 text-muted-foreground">
                      <CheckCircle2 className="mt-0.5 h-3.5 w-3.5 shrink-0 text-primary" aria-hidden="true" />
                      <span>{outcome}</span>
                    </li>
                  ))}
                </ul>
              </div>
            </section>

            <div className="space-y-8">
              {selectedPage.sections.map((section, sectionIndex) => (
                <section key={sectionIndex} aria-labelledby={`guide-section-${sectionIndex}`}>
                  <div className="flex items-start gap-3">
                    <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-secondary text-xs font-bold text-secondary-foreground">
                      {sectionIndex + 1}
                    </span>
                    <div className="min-w-0 flex-1">
                      <h3 id={`guide-section-${sectionIndex}`} className="text-lg font-semibold">{section.title}</h3>
                      <p className="mt-2 text-sm leading-6 text-muted-foreground">{section.body}</p>

                      {section.steps && (
                        <ol className="mt-4 space-y-3">
                          {section.steps.map((step, stepIndex) => (
                            <li key={stepIndex} className="flex gap-3 rounded-lg border border-border bg-muted/30 p-3 text-sm leading-6">
                              <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-primary text-xs font-bold text-primary-foreground">
                                {stepIndex + 1}
                              </span>
                              <span>{step}</span>
                            </li>
                          ))}
                        </ol>
                      )}

                      {section.bullets && (
                        <ul className="mt-4 grid gap-2 sm:grid-cols-2">
                          {section.bullets.map((bullet, bulletIndex) => (
                            <li key={bulletIndex} className="flex gap-2 rounded-lg bg-muted/50 p-3 text-sm leading-5 text-muted-foreground">
                              <ChevronRight className="mt-0.5 h-4 w-4 shrink-0 text-primary" aria-hidden="true" />
                              <span>{bullet}</span>
                            </li>
                          ))}
                        </ul>
                      )}

                      {section.callout && <Callout callout={section.callout} />}
                    </div>
                  </div>
                </section>
              ))}
            </div>

            <section className="rounded-xl border border-border bg-muted/30 p-4 sm:flex sm:items-center sm:justify-between sm:gap-4">
              <div>
                <h3 className="text-sm font-semibold">Practice this in Agora</h3>
                <p className="mt-1 text-xs leading-5 text-muted-foreground">
                  The guide stays available in the sidebar. Return here whenever you need the next step.
                </p>
              </div>
              {destination && (
                <button
                  onClick={() => onNavigateTab(destination.tab)}
                  className="mt-3 inline-flex shrink-0 items-center gap-2 rounded-lg bg-primary px-4 py-2 text-sm font-semibold text-primary-foreground hover:bg-primary/90 sm:mt-0"
                >
                  {destination.label}
                  <ArrowRight className="h-4 w-4" aria-hidden="true" />
                </button>
              )}
            </section>

            <div className="grid gap-3 border-t border-border pt-6 sm:grid-cols-2">
              <PageNavigationButton
                direction="previous"
                target={previousPage}
                onClick={() => goToFlatPage(previousPage)}
              />
              <PageNavigationButton
                direction="next"
                target={nextPage}
                onClick={() => goToFlatPage(nextPage)}
              />
            </div>
          </div>
        </article>
      </div>
    </div>
  );
}

function JourneyCard({
  eyebrow,
  title,
  description,
  onClick,
}: {
  eyebrow: string;
  title: string;
  description: string;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className="group rounded-xl border border-border bg-card p-4 text-left shadow-sm transition-colors hover:border-primary/40 hover:bg-accent"
    >
      <span className="text-[11px] font-bold uppercase tracking-[0.16em] text-primary">{eyebrow}</span>
      <span className="mt-1 flex items-center justify-between gap-3 text-sm font-semibold">
        {title}
        <ArrowRight className="h-4 w-4 shrink-0 text-muted-foreground transition-transform group-hover:translate-x-0.5 group-hover:text-primary" aria-hidden="true" />
      </span>
      <span className="mt-1 block text-xs leading-5 text-muted-foreground">{description}</span>
    </button>
  );
}

function Callout({ callout }: { callout: GuideCallout }) {
  const styles = {
    tip: {
      icon: Lightbulb,
      className: 'border-primary/25 bg-primary/5',
      iconClassName: 'text-primary',
    },
    note: {
      icon: Info,
      className: 'border-blue-500/25 bg-blue-500/5',
      iconClassName: 'text-blue-600 dark:text-blue-400',
    },
    warning: {
      icon: TriangleAlert,
      className: 'border-amber-500/30 bg-amber-500/10',
      iconClassName: 'text-amber-700 dark:text-amber-300',
    },
  }[callout.tone];
  const Icon = styles.icon;

  return (
    <div className={`mt-4 flex gap-3 rounded-lg border p-3 ${styles.className}`}>
      <Icon className={`mt-0.5 h-4 w-4 shrink-0 ${styles.iconClassName}`} aria-hidden="true" />
      <div>
        <p className="text-sm font-semibold">{callout.title}</p>
        <p className="mt-1 text-xs leading-5 text-muted-foreground">{callout.text}</p>
      </div>
    </div>
  );
}

function PageNavigationButton({
  direction,
  target,
  onClick,
}: {
  direction: 'previous' | 'next';
  target: { topicId: string; level: GuideLevel } | null;
  onClick: () => void;
}) {
  if (!target) return <div />;
  const topic = GUIDE_TOPICS.find((candidate) => candidate.id === target.topicId);
  if (!topic) return <div />;
  const isPrevious = direction === 'previous';

  return (
    <button
      onClick={onClick}
      className={`group flex items-center gap-3 rounded-xl border border-border p-3 text-left hover:bg-accent ${isPrevious ? '' : 'sm:text-right'}`}
    >
      {isPrevious && <ArrowLeft className="h-4 w-4 shrink-0 text-muted-foreground group-hover:text-primary" aria-hidden="true" />}
      <span className="min-w-0 flex-1">
        <span className="block text-[11px] font-bold uppercase tracking-[0.14em] text-muted-foreground">
          {isPrevious ? 'Previous' : 'Next'} page
        </span>
        <span className="mt-0.5 block truncate text-sm font-semibold">
          {topic.shortTitle}: {LEVEL_COPY[target.level].label}
        </span>
      </span>
      {!isPrevious && <ArrowRight className="h-4 w-4 shrink-0 text-muted-foreground group-hover:text-primary" aria-hidden="true" />}
    </button>
  );
}
