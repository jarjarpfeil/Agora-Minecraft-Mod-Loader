export type GuideLevel = 'basic' | 'advanced';

export interface GuideCallout {
  tone: 'tip' | 'note' | 'warning';
  title: string;
  text: string;
}

export interface GuideSection {
  title: string;
  body: string;
  steps?: string[];
  bullets?: string[];
  callout?: GuideCallout;
}

export interface GuidePage {
  summary: string;
  outcomes: string[];
  sections: GuideSection[];
}

export interface GuideTopic {
  id: string;
  title: string;
  shortTitle: string;
  category: 'Start' | 'Play' | 'Manage' | 'Recover' | 'Customize' | 'Connect';
  description: string;
  keywords: string[];
  basic: GuidePage;
  advanced: GuidePage;
}

export const GUIDE_TOPICS: GuideTopic[] = [
  {
    id: 'getting-started',
    title: 'Getting started with Agora',
    shortTitle: 'Getting started',
    category: 'Start',
    description: 'Set up Agora, understand what it does, and reach your first playable instance.',
    keywords: ['onboarding', 'setup', 'registry', 'java', 'first launch'],
    basic: {
      summary: 'Agora helps you discover, organize, and recover modded Minecraft installations. It calls each separate installation an instance and keeps risky changes contained to that instance.',
      outcomes: [
        'Complete the first-run setup without needing prior modding knowledge.',
        'Understand the difference between Agora, Minecraft, and the official launcher.',
        'Create a safe first instance and know where to go next.',
      ],
      sections: [
        {
          title: 'What Agora manages',
          body: 'Agora manages the files and settings around modded Minecraft: instances, mods, packs, Java runtimes, snapshots, compatibility checks, and recovery. By default, the official Minecraft launcher still handles game execution and your Microsoft account.',
          bullets: [
            'An instance is an isolated Minecraft setup with its own version, loader, mods, and settings.',
            'The registry is Agora\'s community-curated catalog. Downloading it enables catalog and governance features.',
            'A mod loader, such as Fabric or Quilt, lets Minecraft load mods. The loader belongs to an instance.',
          ],
          callout: {
            tone: 'note',
            title: 'Your existing game is separate',
            text: 'Creating an Agora instance does not replace your normal Minecraft installation. Changes stay inside the selected instance.',
          },
        },
        {
          title: 'Complete first-run setup',
          body: 'The onboarding wizard checks the optional services and local tools Agora can use. You can skip optional connections and change them later in Settings.',
          steps: [
            'Review the service switches. Leave Modrinth on for a larger live catalog, or off to use only Agora-curated data.',
            'Let Agora find Java. If no suitable runtime exists, use the managed Java download.',
            'Connect GitHub only if you want to vote or participate in community governance.',
            'Download the signed registry when prompted.',
            'Finish onboarding and open Browse to choose a pack, or My Instances to build an empty instance.',
          ],
        },
        {
          title: 'A safe first session',
          body: 'For the easiest start, choose a curated pack that matches a current Minecraft version. Packs select a tested group of mods for you and reduce the number of compatibility decisions you need to make.',
          steps: [
            'Open Browse and filter the content type to Pack.',
            'Open a pack, read its description and supported Minecraft versions, then choose Create Instance from Pack.',
            'Keep the suggested loader and memory unless the pack says otherwise.',
            'Launch the new instance. Resolve any health warnings before selecting Launch Anyway.',
            'Play for at least 60 seconds so Agora can establish a last-known-good recovery point.',
          ],
        },
      ],
    },
    advanced: {
      summary: 'Treat onboarding as a policy decision: choose which external services, runtime sources, launch responsibilities, and cached data Agora may use.',
      outcomes: [
        'Configure a minimal-network or fully featured installation intentionally.',
        'Know which setup choices are reversible and where their controls live.',
        'Verify the registry and runtime state before building instances.',
      ],
      sections: [
        {
          title: 'Choose a service boundary',
          body: 'Agora can run with only its cached registry, or enrich results with Modrinth, GitHub governance, Copilot, and an external MCP client. Each integration has a separate purpose and can be disabled independently.',
          bullets: [
            'Modrinth consent enables the feature; Privacy network permission controls whether live API and CDN access is allowed.',
            'GitHub governance sign-in is independent from the GitHub Copilot connection used by the AI Assistant.',
            'Microsoft sign-in is only needed for direct in-app launching and online play. Delegated launch uses the official launcher.',
          ],
        },
        {
          title: 'Validate the local foundation',
          body: 'Before constructing a large instance, verify the cached registry, selected launch mode, Java policy, and network policy. This avoids discovering a blocked download or incompatible runtime halfway through an install plan.',
          steps: [
            'Open Settings and enable Advanced mode if you need Privacy, manual JVM, or MCP controls.',
            'Confirm the Registry status is ready and current.',
            'Choose Automatic, Prompt, or Manual Java management.',
            'Review every enabled endpoint in Privacy, especially registry, Modrinth, loader, Mojang, authentication, and runtime access.',
            'Create a small test instance and launch it once before importing a large pack.',
          ],
        },
        {
          title: 'Reset or revisit onboarding choices',
          body: 'Onboarding is not a permanent configuration contract. The Services, Accounts, Launching, Java, Updates, and Privacy sections expose the same operational choices with more detail.',
          callout: {
            tone: 'tip',
            title: 'Prefer explicit checks',
            text: 'When preparing an offline machine, download the registry, loader metadata, required mods, and Java runtime before enabling Lockdown mode.',
          },
        },
      ],
    },
  },
  {
    id: 'modding-foundations',
    title: 'Modding foundations',
    shortTitle: 'Modding basics',
    category: 'Start',
    description: 'Learn versions, loaders, dependencies, and the different kinds of Minecraft add-ons.',
    keywords: ['minecraft version', 'loader', 'fabric', 'quilt', 'dependencies', 'mods'],
    basic: {
      summary: 'Most modding problems come from mixing the wrong Minecraft version, loader, or dependency. Agora exposes these details so you can make compatible choices before files are changed.',
      outcomes: [
        'Read a compatibility label and version list confidently.',
        'Know what each content type changes.',
        'Recognize dependencies and conflicts before installing.',
      ],
      sections: [
        {
          title: 'The compatibility triangle',
          body: 'A mod normally has to match three things: the Minecraft version, the mod loader, and the versions of other mods it depends on. A Fabric mod for Minecraft 1.21 should not be assumed to work on Quilt, NeoForge, or Minecraft 1.20.1 unless its version page explicitly says so.',
          bullets: [
            'Minecraft version: the game release the file was built for.',
            'Loader: the platform that discovers and starts mods.',
            'Dependencies: other mods or libraries required by the selected file.',
          ],
        },
        {
          title: 'Know the content types',
          body: 'Agora can discover more than code mods. Select the content type that matches the result you want.',
          bullets: [
            'Mods change game code or behavior and usually require a loader.',
            'Packs create a complete instance from a coordinated set of content.',
            'Resource packs change textures, sounds, fonts, or models.',
            'Shaders change rendering and usually require a compatible shader-support mod.',
            'Data packs change recipes, loot, world generation, or game rules for worlds.',
            'Worlds provide playable world data; servers provide multiplayer destinations or community information.',
          ],
        },
        {
          title: 'Read compatibility before popularity',
          body: 'A popular project can still be wrong for your instance. Start from an instance-aware Browse search, look for Compatible, inspect the Versions page, and review the install plan before applying it.',
          callout: {
            tone: 'warning',
            title: 'Major match is not a guarantee',
            text: 'A major-version match is weaker than an exact compatible result. Back up the instance and expect to test it.',
          },
        },
      ],
    },
    advanced: {
      summary: 'Compatibility is a graph, not a label. Advanced modding means reasoning about exact artifacts, transitive dependencies, optional integrations, loader profiles, and configuration state.',
      outcomes: [
        'Evaluate an install plan beyond its top-level mod.',
        'Understand why a file can pass basic filtering and still fail at runtime.',
        'Build repeatable test habits for complex mod sets.',
      ],
      sections: [
        {
          title: 'Think in resolved artifacts',
          body: 'A project name is not the installed unit. The installed unit is a specific file with a source, game version, loader set, and hash. When troubleshooting, record the exact artifact rather than saying only that a project is installed.',
          bullets: [
            'Required dependencies must resolve before the operation can proceed.',
            'Optional dependencies may unlock integrations but also enlarge the compatibility surface.',
            'Conflicts may be declared by curators, project metadata, or the local health scanner.',
            'A file copied in manually may have less metadata than a file installed through Agora.',
          ],
        },
        {
          title: 'Version ranges and build metadata',
          body: 'Loaders and projects use different version schemes. Do not compare versions as plain text. Prefer Agora\'s exact compatibility result and the publisher\'s version metadata. Fabric build metadata such as +mc1.21.1 describes the target but is not a newer semantic version by itself.',
        },
        {
          title: 'Change one variable at a time',
          body: 'For large instances, treat changes as experiments. Create a snapshot, install one coherent group, launch, and confirm stability before continuing. Loadout profiles can isolate feature groups without deleting files.',
          steps: [
            'Start from a known-good snapshot.',
            'Apply a reviewed install plan.',
            'Launch and exercise the affected game area.',
            'If stable, name a new snapshot or let a successful session promote the last-known-good state.',
            'If unstable, restore and revise the dependency or version choice.',
          ],
        },
      ],
    },
  },
  {
    id: 'home-navigation',
    title: 'Home and navigation',
    shortTitle: 'Home & navigation',
    category: 'Start',
    description: 'Use the dashboard, sidebar, alerts, recommendations, and keyboard navigation.',
    keywords: ['home', 'sidebar', 'command palette', 'quick actions', 'recommendations'],
    basic: {
      summary: 'The Home page prioritizes what needs attention, what you played last, recovery options, and compatible discoveries. The sidebar keeps every major area one click away.',
      outcomes: [
        'Recognize each Home dashboard section.',
        'Move quickly between the catalog, instances, community, guide, and settings.',
        'Use the command palette without reaching for the mouse.',
      ],
      sections: [
        {
          title: 'Read Home from top to bottom',
          body: 'Alerts appear first because they may need action. Continue Playing launches the most recent instance. Last Known Good offers recovery when one exists. Compatible recommendations use your recent instance version and loader.',
          bullets: [
            'A registry alert means discovery data is missing or unavailable.',
            'A crash alert means the latest instance did not exit cleanly.',
            'Recommendations are filtered for your active instance when possible; they are not paid placements.',
          ],
        },
        {
          title: 'Use the sidebar',
          body: 'Home is the dashboard, Browse is the catalog, My Instances is your library, Community Governance shows curation activity, Help & Guide is this learning center, and Settings controls Agora itself. AI Assistant appears when that service is enabled.',
          steps: [
            'Select an item to move to that area.',
            'Use the arrow button at the sidebar edge to collapse or expand it.',
            'Hover a collapsed icon to see its name.',
          ],
        },
        {
          title: 'Open Quick actions',
          body: 'Press Ctrl+K on Windows and Linux, or Cmd+K on macOS. Type an instance or destination name, use the arrow keys to select it, and press Enter. Escape closes the palette.',
          callout: {
            tone: 'tip',
            title: 'Text fields keep their shortcuts',
            text: 'The palette shortcut is ignored while you are typing in an input, text area, or editable field.',
          },
        },
      ],
    },
    advanced: {
      summary: 'Customize the application shell for dense workflows and understand how Home derives its alerts, recovery state, and recommendations.',
      outcomes: [
        'Resize and reset the sidebar efficiently.',
        'Interpret recommendation and recovery cards precisely.',
        'Use navigation history without losing Browse or editor context.',
      ],
      sections: [
        {
          title: 'Tune the shell',
          body: 'Drag the sidebar divider between its minimum and maximum width. Use Left or Right while the divider is focused for keyboard resizing, hold Shift for larger increments, and double-click the divider to reset it. Layout choices persist locally.',
        },
        {
          title: 'Understand Home signals',
          body: 'The Last Known Good card points to an explicitly promoted pre-launch snapshot, not merely the newest snapshot. Recommendations rank category overlap and then filter against the active instance\'s Minecraft version and loader.',
          callout: {
            tone: 'note',
            title: 'Recovery is deliberate',
            text: 'Restoring a last-known-good state creates an undo snapshot first, so the current state is not silently discarded.',
          },
        },
        {
          title: 'Preserved work context',
          body: 'Opening an item from Browse or the Instance Editor preserves the source page and scroll position. Use Back to return to the same working context. The browser-style back action is part of Agora\'s navigation history even though the app has no traditional web URLs.',
        },
      ],
    },
  },
  {
    id: 'instances',
    title: 'Creating and managing instances',
    shortTitle: 'Instances',
    category: 'Play',
    description: 'Create isolated setups, edit their content, lock stable builds, and manage their lifecycle.',
    keywords: ['instance', 'create', 'edit', 'delete', 'lock', 'memory'],
    basic: {
      summary: 'An instance is a self-contained Minecraft setup. Separate instances let you keep different game versions, mod loaders, packs, and play styles without mixing their files.',
      outcomes: [
        'Create an instance with compatible core settings.',
        'Open, launch, rename, lock, and delete an instance safely.',
        'Know when to use separate instances instead of reconfiguring one.',
      ],
      sections: [
        {
          title: 'Create an instance',
          body: 'Open My Instances and select Create Instance. Give it a recognizable name, then choose a Minecraft version, loader, loader version, and memory allocation.',
          steps: [
            'Choose the Minecraft version required by the mods or pack you intend to use.',
            'Choose one loader. Do not mix loader-specific mod files.',
            'Keep the suggested loader version unless a project requires another.',
            'Start with a moderate memory allocation and adjust only when needed.',
            'Create the instance, then use Edit to add content.',
          ],
        },
        {
          title: 'Use instance cards',
          body: 'Each card shows the instance name, loader, Minecraft version, last launch, and lock state. Launch starts the game, Edit opens the full editor, Troubleshoot opens diagnostic tools, and Delete permanently removes the instance after confirmation.',
          callout: {
            tone: 'warning',
            title: 'Export before deleting',
            text: 'Snapshots are local recovery points, not an external backup. Export anything you need before deleting an instance.',
          },
        },
        {
          title: 'Lock a stable setup',
          body: 'Locking prevents content changes while still allowing launch. Use it after a pack or personal setup is working well. Unlock only when you intend to install, remove, enable, disable, or update content.',
        },
      ],
    },
    advanced: {
      summary: 'Use instances as controlled environments: establish a baseline, isolate purpose-specific builds, and manage mutable and reproducible states intentionally.',
      outcomes: [
        'Design an instance strategy for testing and long-term play.',
        'Use locks and snapshots as separate safeguards.',
        'Interpret running, provisioning, and recoverable-profile states.',
      ],
      sections: [
        {
          title: 'Separate by compatibility boundary',
          body: 'Create a new instance when changing Minecraft major/minor versions, switching loaders, evaluating an uncertain pack, or maintaining distinct server requirements. Reusing one instance across those boundaries makes rollback and diagnosis harder.',
          bullets: [
            'Keep a locked, known-good play instance.',
            'Use a disposable test instance for major updates and unverified manual files.',
            'Clone from a lockfile when exact reproducibility matters.',
          ],
        },
        {
          title: 'Locking versus snapshots',
          body: 'A lock blocks future content edits; a snapshot captures a restorable state. Use both for important instances: snapshot the baseline, then lock it. Unlocking does not erase the snapshot.',
        },
        {
          title: 'Read operational states',
          body: 'The Instances page can show Java provisioning progress, a running direct-launch process and PID, recent console output, or a recoverable launcher-profile warning. Prefer Reinstall Loader for a damaged profile; use delegated launch when you want the official launcher to own execution.',
          callout: {
            tone: 'warning',
            title: 'Kill is a last resort',
            text: 'Terminating a running process can lose unsaved game data. Exit Minecraft normally whenever possible.',
          },
        },
      ],
    },
  },
  {
    id: 'browse-registry',
    title: 'Browse, search, and the registry',
    shortTitle: 'Browse & registry',
    category: 'Play',
    description: 'Find curated and live content, filter for compatibility, and understand trust labels.',
    keywords: ['browse', 'search', 'catalog', 'registry', 'curated', 'modrinth', 'sort'],
    basic: {
      summary: 'Browse combines Agora\'s curated registry with optional live Modrinth results. Instance-aware discovery is the easiest way to avoid incompatible downloads.',
      outcomes: [
        'Search and filter the catalog effectively.',
        'Find content compatible with a specific instance.',
        'Understand Curated, Compatible, Major Match, and Installed labels.',
      ],
      sections: [
        {
          title: 'Start with the right scope',
          body: 'Use the content type filter for mods, packs, shaders, resource packs, servers, data packs, or worlds. Add a search term and category only when needed; overly narrow filters can hide valid results.',
          steps: [
            'Open Browse.',
            'Choose Discover for an instance and select the instance you plan to change.',
            'Choose a content type.',
            'Search by project name, feature, or category.',
            'Open a result to inspect its About, Gallery, Versions, and Agora information.',
          ],
        },
        {
          title: 'Read result badges',
          body: 'Compatible means Agora found an appropriate version for the selected Minecraft version and loader. Major Match is less exact and deserves testing. Installed means the selected instance already contains the project. Curated identifies a community-reviewed Agora registry entry.',
        },
        {
          title: 'Choose a sort',
          body: 'For You uses overlap with your active instance. Net Score reflects community balance, Trending reflects recent voting velocity, Newest emphasizes recently added entries, and vote sorts expose the strongest positive or negative community signals.',
          callout: {
            tone: 'note',
            title: 'Curated and live results are different',
            text: 'A result without the Curated badge may still be legitimate, but it comes from the enabled live source rather than Agora\'s reviewed registry.',
          },
        },
      ],
    },
    advanced: {
      summary: 'Use catalog provenance, compatibility metadata, and ranking behavior to evaluate results rather than treating search order as a recommendation guarantee.',
      outcomes: [
        'Distinguish cached registry data from live provider data.',
        'Interpret discovery ranking and degraded/offline states.',
        'Audit a project before it reaches an instance.',
      ],
      sections: [
        {
          title: 'Understand data provenance',
          body: 'Curated entries ship in the signed Agora registry. Optional Modrinth results are requested live when both service consent and network permission are enabled. Detail pages may merge cached annotations with live descriptions, galleries, and versions.',
          bullets: [
            'Cached data remains available offline.',
            'Live versions can be newer than the registry but have a different trust path.',
            'The project source and selected artifact hash matter more than the visual card alone.',
          ],
        },
        {
          title: 'Audit before install',
          body: 'Check the license, source link, update date, supported loaders, exact game versions, dependency list, curator notes, reviews, and version changelog. Immunity indicates community protection from ordinary removal pressure; it is not a technical compatibility certification.',
        },
        {
          title: 'Work through degraded states',
          body: 'When the registry is loading, missing, or offline, read the status panel before assuming no content exists. Download or refresh the registry when allowed. In Lockdown mode, expect only already cached data and locally available artifacts.',
          callout: {
            tone: 'tip',
            title: 'Use reproducible references',
            text: 'When sharing a result with another player, identify the exact version or export a pack or lockfile instead of relying on search position.',
          },
        },
      ],
    },
  },
  {
    id: 'install-update',
    title: 'Installing, removing, and updating mods',
    shortTitle: 'Install & update',
    category: 'Play',
    description: 'Review dependency-aware plans and make content changes with recovery protection.',
    keywords: ['install flow', 'dependencies', 'remove', 'updates', 'rollback', 'conflicts'],
    basic: {
      summary: 'Agora plans content changes before touching the instance. The review screen explains added dependencies, conflicts, file changes, and the snapshot that protects the operation.',
      outcomes: [
        'Install a project into the correct instance.',
        'Review dependencies and resolve conflicts.',
        'Update or remove content without bypassing recovery.',
      ],
      sections: [
        {
          title: 'Install from a detail page',
          body: 'Select Install to Instance, choose the target, select a compatible project version, and review the plan. Packs use Create Instance from Pack because they define a complete starting setup.',
          steps: [
            'Confirm the target instance, Minecraft version, and loader.',
            'Choose an exact-compatible file when available.',
            'Review required and optional dependencies.',
            'Resolve every conflict shown in the plan.',
            'Check the snapshot and file summary, then apply the operation.',
          ],
        },
        {
          title: 'Remove safely',
          body: 'Use Remove from the Mods tab instead of deleting the file manually. Agora can identify dependents, stage the removal, create a recovery snapshot, and run health checks afterward.',
          callout: {
            tone: 'warning',
            title: 'Dependents may stop loading',
            text: 'If another mod requires the one you remove, remove or replace the dependent as part of the reviewed plan.',
          },
        },
        {
          title: 'Check for updates',
          body: 'From My Instances, select Check for Updates. Review the updates per unlocked instance, deselect anything you want to postpone, then review and apply the batch plan.',
          bullets: [
            'Locked instances are skipped.',
            'A newer version is not automatically compatible with every other mod.',
            'The result screen reports success, cancellation, failure, or automatic health rollback.',
          ],
        },
      ],
    },
    advanced: {
      summary: 'The Install Flow is a transaction planner. Use its dependency graph, conflict choices, snapshot boundary, hash checks, and post-apply health result to reason about complex changes.',
      outcomes: [
        'Evaluate and revise a resolved install plan.',
        'Use batch operations without losing atomic recovery.',
        'Know what cancellation and health rollback guarantee.',
      ],
      sections: [
        {
          title: 'Inspect the resolved plan',
          body: 'The plan is computed from the intent, selected artifact, dependencies, optional choices, conflicts, current files, and instance state. Changing an optional dependency or conflict resolution causes the plan to be resolved again before execution.',
          bullets: [
            'Blockers must be fixed before execution.',
            'Warnings deserve review but may be acceptable for a controlled test.',
            'File changes show the operation\'s actual scope, not only the top-level project.',
          ],
        },
        {
          title: 'Understand the transaction boundary',
          body: 'Agora creates a recovery snapshot, downloads and verifies artifacts, applies the planned changes, then runs health validation. If the final state has health blockers, Agora restores the recovery snapshot automatically.',
          callout: {
            tone: 'note',
            title: 'Health rollback is not gameplay testing',
            text: 'Static health checks cannot prove that a mod set is stable in every world. Launch and test the affected behavior before treating the change as known good.',
          },
        },
        {
          title: 'Batch changes strategically',
          body: 'Group updates that belong to one compatibility family, such as a library and its dependents. Avoid updating every subsystem at once on a large instance. Smaller batches produce clearer diffs and make crash investigation more decisive.',
          steps: [
            'Snapshot the known-good baseline.',
            'Select one coherent update group.',
            'Review changed artifacts and hashes.',
            'Apply, launch, and test.',
            'Continue only after the instance is stable.',
          ],
        },
      ],
    },
  },
  {
    id: 'content-management',
    title: 'Managing mods and game content',
    shortTitle: 'Manage content',
    category: 'Manage',
    description: 'Organize mods, resource packs, shaders, and data packs inside an instance.',
    keywords: ['mods tab', 'resource packs', 'shaders', 'data packs', 'enable', 'disable', 'manual jar'],
    basic: {
      summary: 'The Instance Editor separates content by type and shows what is installed. Use Agora\'s controls so changes remain visible to snapshots, health checks, and recovery tools.',
      outcomes: [
        'Find and manage each kind of installed content.',
        'Temporarily disable a mod without deleting it.',
        'Add local files with appropriate caution.',
      ],
      sections: [
        {
          title: 'Use the content tabs',
          body: 'Open My Instances, select Edit, then choose Mods, Resource Packs, Shaders, or Data Packs. Each list shows the installed file and available actions for that content type.',
          bullets: [
            'View Details returns to the project page when Agora can resolve the source.',
            'Disable keeps a mod file available but prevents it from loading.',
            'Remove starts a reviewed removal plan.',
            'Add opens Browse with the current instance and content type already selected.',
          ],
        },
        {
          title: 'Enable and disable for testing',
          body: 'Disabling is useful when diagnosing a conflict or keeping optional features available for later. Relaunch after changing enabled state; Minecraft cannot unload most mods from a running game.',
        },
        {
          title: 'Import a local file',
          body: 'Use Import Mod or the drop zone for a local JAR. Use the corresponding content picker for supported resource packs, shaders, and data packs. Confirm the file came from a trusted source and matches the instance.',
          callout: {
            tone: 'warning',
            title: 'Manual files have fewer guarantees',
            text: 'A manually supplied file may lack source metadata and curated compatibility information. Create a snapshot first and never run untrusted JAR files.',
          },
        },
      ],
    },
    advanced: {
      summary: 'Manage content as a traceable inventory with source metadata, hashes, enabled state, and controlled manual exceptions.',
      outcomes: [
        'Identify source and metadata gaps in an instance.',
        'Use content state to run controlled compatibility tests.',
        'Avoid configuration and world-data pitfalls when changing content.',
      ],
      sections: [
        {
          title: 'Read the installed inventory',
          body: 'The Mods list distinguishes the resolved project name from the physical filename and records source and install time when available. A missing registry or provider identity is a signal that future automated updates may require manual attention.',
        },
        {
          title: 'Design controlled mod groups',
          body: 'Use enabled state and loadout profiles to separate performance, visual, content, and debugging groups. Keep required libraries enabled with their dependents. Record a named snapshot before changing core libraries or world-generation mods.',
        },
        {
          title: 'Protect persistent game data',
          body: 'Removing a world-generation, storage, or content mod can leave missing blocks, items, or data in a save even if the game launches. Back up important worlds separately and test removal on a copy.',
          callout: {
            tone: 'warning',
            title: 'A clean health scan is not a save migration',
            text: 'Agora can validate files and known compatibility rules, but it cannot guarantee that removing content preserves every world safely.',
          },
        },
      ],
    },
  },
  {
    id: 'launching',
    title: 'Launching Minecraft and health checks',
    shortTitle: 'Launch & health',
    category: 'Play',
    description: 'Choose a launch mode, resolve preflight findings, and understand running-state controls.',
    keywords: ['launch', 'delegated', 'direct', 'health check', 'console', 'running'],
    basic: {
      summary: 'Agora checks an instance before launch, then either hands it to the official launcher or starts Minecraft directly, depending on your Settings choice.',
      outcomes: [
        'Choose the appropriate launch mode.',
        'Respond safely to health warnings and blockers.',
        'Know where to find progress and console information.',
      ],
      sections: [
        {
          title: 'Choose delegated or direct launch',
          body: 'Delegated launch is the default: Agora prepares and selects the profile, then the official launcher starts Minecraft. Direct launch keeps execution and console output inside Agora and requires Microsoft sign-in for full online play.',
          bullets: [
            'Use delegated launch for the simplest account and launcher setup.',
            'Use direct launch when you need in-app process status and console output.',
            'Change the mode in Settings under Launching.',
          ],
        },
        {
          title: 'Handle the health screen',
          body: 'A blocker should be repaired before launch. A warning explains a risk you may choose to accept. Agora may offer to disable a problematic mod directly from the dialog.',
          steps: [
            'Read the finding and affected file.',
            'Repair missing dependencies or incompatible Java first.',
            'Disable a suspect only if you understand the dependent impact.',
            'Use Launch Anyway only for a deliberate, recoverable test.',
          ],
        },
        {
          title: 'During and after launch',
          body: 'Provisioning progress appears while Agora prepares Java or loader files. Direct launch displays a running state and game console. Exit normally from Minecraft whenever possible so saves and the recovery lifecycle complete cleanly.',
        },
      ],
    },
    advanced: {
      summary: 'Use launch mode, health policy, muted findings, process state, and console output as a controlled execution workflow rather than a single Play action.',
      outcomes: [
        'Understand preflight and runtime responsibilities.',
        'Use warning muting without hiding new classes of problems.',
        'Capture useful evidence from direct-launch failures.',
      ],
      sections: [
        {
          title: 'Separate preflight from runtime',
          body: 'Health checks can catch known incompatibilities, missing dependencies, and invalid runtime choices before process start. They cannot prove runtime behavior, graphics-driver compatibility, world integrity, or interactions that appear only during play.',
        },
        {
          title: 'Use muted findings narrowly',
          body: 'Muting is stored by warning kind and affected mod so known acceptable findings do not interrupt every launch. If every finding is muted, Agora proceeds automatically. Revisit muted warnings after major updates instead of treating them as permanent truth.',
          callout: {
            tone: 'warning',
            title: 'Do not mute unexplained blockers',
            text: 'A muted message is not a fix. Keep a snapshot and document why the finding is safe for this exact instance.',
          },
        },
        {
          title: 'Collect direct-launch evidence',
          body: 'Use the Console tab to filter INFO, WARN, ERROR, or DEBUG output, then copy the relevant sequence. Preserve the earliest error and its surrounding context; the last line is often only a consequence. Pair console output with the generated crash report when available.',
        },
      ],
    },
  },
  {
    id: 'crash-recovery',
    title: 'Crash diagnosis and recovery',
    shortTitle: 'Crashes & recovery',
    category: 'Recover',
    description: 'Use Crash Doctor, evidence-based testing, and last-known-good recovery.',
    keywords: ['crash doctor', 'crash report', 'suspect', 'disable relaunch', 'restore'],
    basic: {
      summary: 'Crashes are usually solved fastest by preserving the current state, reading the first useful evidence, and testing one likely cause at a time. Crash Doctor guides that process.',
      outcomes: [
        'Open an automatic or pasted-log investigation.',
        'Test a ranked suspect without losing the starting state.',
        'Restore a known-good setup when investigation is not the priority.',
      ],
      sections: [
        {
          title: 'Start an investigation',
          body: 'A recent crash can appear as a Home alert or on an instance card. Open Troubleshoot or paste a log when automatic detection cannot find the report. Crash Doctor ranks likely suspects from available signals.',
        },
        {
          title: 'Test the top suspect',
          body: 'Select Disable & Relaunch. Agora creates a recovery snapshot before changing the enabled state. After testing, choose Yes, fixed or Still crashing so the investigation can record the result and continue.',
          steps: [
            'Read the suspect name and evidence summary.',
            'Review any dependent mods that would also be affected.',
            'Disable the smallest safe set and relaunch.',
            'Reproduce the action that caused the crash.',
            'Report the result accurately rather than changing several other things first.',
          ],
        },
        {
          title: 'Recover instead of investigate',
          body: 'If you need to play immediately, restore the Last Known Good state from Home or the Snapshots tab. Use Restore All & Close in Crash Doctor to undo its test changes.',
          callout: {
            tone: 'tip',
            title: 'Keep the crash report',
            text: 'Restoring files may remove the immediate problem, but the original report is still valuable when reporting the incompatibility upstream.',
          },
        },
      ],
    },
    advanced: {
      summary: 'Treat crash diagnosis as hypothesis testing. Use signal weights, dependency impact, ruled-out suspects, fingerprints, and preserved baselines to converge without contaminating evidence.',
      outcomes: [
        'Interpret why Crash Doctor ranked a suspect.',
        'Run dependency-aware tests with minimal variables.',
        'Produce a useful report when local diagnosis is inconclusive.',
      ],
      sections: [
        {
          title: 'Read the signal breakdown',
          body: 'A suspect score can combine stack-frame presence, curated conflicts, dependency relationships, prior local crashes, and confirmed fixes. A high score is a hypothesis, not proof. Ruled-out mods stay visible so the investigation history is auditable.',
        },
        {
          title: 'Control the experiment',
          body: 'Accept only the dependent disable set required for a valid test. Do not update Java, replace the loader, remove extra mods, and change JVM flags at the same time. If the crash disappears, confirm the fix before restoring unrelated features.',
        },
        {
          title: 'Escalate with a complete evidence set',
          body: 'Include the exact Minecraft and loader versions, mod file names, crash fingerprint or report, steps to reproduce, whether a clean world also fails, and the suspect tests already completed. Use a reproduction lockfile when sharing the exact file state is important.',
          callout: {
            tone: 'note',
            title: 'AI analysis leaves the machine',
            text: 'Explain with AI sends crash context to the configured AI provider. Review the Privacy guidance before using it with logs that may contain personal paths or server details.',
          },
        },
      ],
    },
  },
  {
    id: 'snapshots-loadouts',
    title: 'Snapshots and loadout profiles',
    shortTitle: 'Snapshots & loadouts',
    category: 'Recover',
    description: 'Create restore points and switch between groups of enabled mods.',
    keywords: ['snapshot', 'last known good', 'loadout', 'profile', 'diff', 'restore'],
    basic: {
      summary: 'Snapshots restore files to an earlier state. Loadout profiles remember which installed mods are enabled. They solve different problems and work well together.',
      outcomes: [
        'Create, compare, restore, and delete snapshots.',
        'Understand automatic last-known-good promotion.',
        'Save and apply an enabled-mod loadout.',
      ],
      sections: [
        {
          title: 'Create a snapshot',
          body: 'Open an instance, select Snapshots, enter a useful label, and create the snapshot before a risky change. Include the reason in the label, such as Before 1.21.2 update or Stable server setup.',
        },
        {
          title: 'Compare and restore',
          body: 'Show diff lists added, removed, and modified files relative to the snapshot. Restore returns the instance to that state and first creates an undo snapshot. Delete removes only the selected recovery point.',
          callout: {
            tone: 'note',
            title: 'Last Known Good is automatic',
            text: 'After a successful play session of at least 60 seconds, Agora can promote the exact pre-launch snapshot as the current Last Known Good state.',
          },
        },
        {
          title: 'Create a loadout profile',
          body: 'Open Loadout Profiles, name the current enabled-mod arrangement, and save it. Apply another profile to switch enabled states without removing files. Relaunch Minecraft after applying a profile.',
        },
      ],
    },
    advanced: {
      summary: 'Use snapshot diffs and loadout profiles as lightweight change management: recover file state, isolate feature sets, and preserve an auditable baseline.',
      outcomes: [
        'Interpret drift beyond simple mod counts.',
        'Build test matrices from profiles and snapshots.',
        'Know the storage and backup limits of both features.',
      ],
      sections: [
        {
          title: 'Read drift precisely',
          body: 'A diff reports added, removed, and modified paths. An enabled-to-disabled transition may appear as a removed JAR plus an added .disabled file. Configuration drift can matter even when the mod inventory is unchanged.',
        },
        {
          title: 'Combine baselines and profiles',
          body: 'Use a named stable snapshot as the file baseline, then create profiles for client visuals, multiplayer-safe content, debugging, or performance testing. Profiles do not revert versions or configuration; restore the snapshot when those must also return to baseline.',
        },
        {
          title: 'Plan external backup',
          body: 'Snapshots and profiles live in Agora\'s local state and instance storage. They are not cloud sync and should not be your only backup for valuable worlds. Export a pack or lockfile for reconstruction and back up irreplaceable saves separately.',
          callout: {
            tone: 'warning',
            title: 'A lockfile is not a world backup',
            text: 'Reproduction data describes content and settings, not the private contents of your saves and configuration files.',
          },
        },
      ],
    },
  },
  {
    id: 'packs-sharing',
    title: 'Packs, import, export, and reproduction',
    shortTitle: 'Packs & sharing',
    category: 'Manage',
    description: 'Move complete setups between machines and verify that another instance matches.',
    keywords: ['pack', 'mrpack', 'import', 'export', 'lockfile', 'clone', 'repair'],
    basic: {
      summary: 'Packs are the easiest way to distribute a coordinated setup. Agora supports Modrinth packs and its native pack format, while reproduction lockfiles focus on proving exact file state.',
      outcomes: [
        'Import a supported pack into a new instance.',
        'Choose the right export format.',
        'Share a setup without assuming snapshots travel with it.',
      ],
      sections: [
        {
          title: 'Import a pack',
          body: 'Use Import Pack from the instance area or Import in the editor. Select a .mrpack or supported Agora pack file. Agora prepares the loader and required content before promoting the new instance.',
          steps: [
            'Choose the pack file from a trusted source.',
            'Review the new instance name and target location.',
            'Leave save symlinking off unless you understand how shared save paths behave.',
            'Wait for preparation and health validation to finish.',
            'Launch once and verify the pack before adding more content.',
          ],
        },
        {
          title: 'Choose an export',
          body: 'Use Modrinth Pack (.mrpack) for broad compatibility. Use Agora Pack (.json) for Agora-native metadata. The exported file location opens in your system file manager after a successful export.',
        },
        {
          title: 'Share responsibly',
          body: 'Tell recipients which Minecraft and loader versions the setup targets. Do not assume private saves, server lists, account details, or all configuration are included. Confirm that every bundled project license permits redistribution.',
          callout: {
            tone: 'warning',
            title: 'Test the export',
            text: 'Import the exported pack into a fresh instance before relying on it as a distributable artifact.',
          },
        },
      ],
    },
    advanced: {
      summary: 'Use reproduction lockfiles to verify, repair, or clone exact artifact state while keeping private file contents out of the shared description.',
      outcomes: [
        'Distinguish a distributable pack from a diagnostic lockfile.',
        'Verify and repair drift transactionally.',
        'Build a reliable reproduction artifact for support or testing.',
      ],
      sections: [
        {
          title: 'Understand reproduction lockfiles',
          body: 'A lockfile records artifacts, sources, hashes, and relevant settings so another instance can be compared with the original. It avoids embedding private configuration contents. Export, copy, or paste one from the Export tab.',
        },
        {
          title: 'Verify, repair, or clone',
          body: 'Verify compares the current instance against the lockfile. Repair snapshots the instance, downloads exact expected artifacts, removes extras, and rolls back if health validation fails. Clone builds a separate instance from the described state.',
          steps: [
            'Validate that the lockfile came from a trusted person or project.',
            'Run Verify and review every missing, extra, or changed file.',
            'Snapshot valuable local changes before Repair.',
            'Prefer Clone when you need to preserve the original instance untouched.',
            'Reproduce the issue before making additional changes.',
          ],
        },
        {
          title: 'Build support-grade reproductions',
          body: 'Minimize the instance while preserving the problem, verify it against the final lockfile, and document exact reproduction steps. Keep worlds and logs separate when they are necessary, and review them for private data before sharing.',
          callout: {
            tone: 'tip',
            title: 'Hashes identify bytes, not trust',
            text: 'A matching hash proves two files are identical. It does not prove that the original file was safe or correctly licensed.',
          },
        },
      ],
    },
  },
  {
    id: 'java-performance',
    title: 'Java, memory, and performance',
    shortTitle: 'Java & performance',
    category: 'Customize',
    description: 'Select a compatible runtime and tune memory and garbage collection without guesswork.',
    keywords: ['java', 'jvm', 'memory', 'ram', 'garbage collection', 'g1gc', 'zgc'],
    basic: {
      summary: 'Minecraft versions require compatible Java versions. Agora can discover or download runtimes and select sensible garbage-collection behavior automatically.',
      outcomes: [
        'Use a compatible Java runtime.',
        'Allocate enough memory without starving the computer.',
        'Keep automatic GC tuning unless evidence supports a change.',
      ],
      sections: [
        {
          title: 'Let Agora manage Java',
          body: 'Automatic runtime mode selects a compatible detected or managed runtime. Managed downloads are stored for Agora and do not change your system PATH. Use Prompt if you want approval before downloads, or Manual when you maintain runtimes yourself.',
        },
        {
          title: 'Set memory conservatively',
          body: 'More memory is not always faster. Allocate enough for the pack while leaving several gigabytes for the operating system, launcher, browser, and graphics driver. Increase memory when logs show genuine heap pressure, not only because a pack feels slow.',
          callout: {
            tone: 'tip',
            title: 'Start with the pack recommendation',
            text: 'Small setups often need much less memory than large content packs. Change one step at a time and observe actual behavior.',
          },
        },
        {
          title: 'Keep GC on Auto',
          body: 'Auto chooses the supported collector and flags for the selected Java runtime. On Java 21 or newer it can use Generational ZGC; older supported runtimes receive tuned G1GC behavior. The launch preview shows the effective result.',
        },
      ],
    },
    advanced: {
      summary: 'Override Java and JVM behavior per instance only when measurement or a specific compatibility requirement justifies departing from automatic policy.',
      outcomes: [
        'Inspect and override runtimes safely.',
        'Compare GC modes and pre-touch tradeoffs.',
        'Diagnose JVM flags from the effective launch preview.',
      ],
      sections: [
        {
          title: 'Manage runtime precedence',
          body: 'Settings controls global runtime policy and discovered runtimes. Java & Args can override the executable for one instance. Inspect an override before saving it and clear it when the experiment ends so automatic compatibility checks regain control.',
        },
        {
          title: 'Choose GC intentionally',
          body: 'Auto is the default. Low-latency ZGC can reduce long pauses with a compatible modern Java. High-efficiency G1GC is an explicit manual profile. AlwaysPreTouch commits heap pages at startup, which can reduce later stutter but increases startup time and immediate memory pressure.',
          bullets: [
            'Compare the same world, route, and workload.',
            'Track pause behavior, startup time, total memory, and stability.',
            'Do not combine collector flags from different GC families.',
          ],
        },
        {
          title: 'Use manual flags sparingly',
          body: 'Manual arguments can override safe defaults or duplicate computed flags. Read the launch preview and remove stale tuning copied from older Java versions. Use Allow incompatible Java override only for a controlled test with a restorable snapshot.',
          callout: {
            tone: 'warning',
            title: 'JVM flag lists age quickly',
            text: 'A popular flag set for Java 8 may be invalid or harmful on Java 17, 21, or 25. Prefer the current automatic profile.',
          },
        },
      ],
    },
  },
  {
    id: 'settings-appearance',
    title: 'Settings, appearance, and accessibility',
    shortTitle: 'Settings & appearance',
    category: 'Customize',
    description: 'Adjust Agora to your display, input, accessibility, and update preferences.',
    keywords: ['settings', 'appearance', 'theme', 'density', 'motion', 'contrast', 'updates'],
    basic: {
      summary: 'Settings changes how Agora looks and behaves. Appearance controls are independent from Minecraft graphics settings and can be reset without changing instances.',
      outcomes: [
        'Apply a coherent appearance preset.',
        'Adjust text, contrast, motion, spacing, and sidebar layout.',
        'Find service, account, launch, Java, launcher, and update controls.',
      ],
      sections: [
        {
          title: 'Start with a preset',
          body: 'Choose an appearance preset closest to your needs, then adjust color mode, accent, font, density, corners, and text scale. Presets include default, night, compact, and high-readability options.',
        },
        {
          title: 'Improve comfort and access',
          body: 'Use High contrast for stronger boundaries, Reduce motion to minimize animation, Text scale for typography, and Density for control spacing. These settings are separate so larger text does not have to make every panel excessively spacious.',
          callout: {
            tone: 'tip',
            title: 'System settings are respected',
            text: 'System color and motion modes can follow operating-system preferences. Full motion deliberately overrides reduced-motion behavior.',
          },
        },
        {
          title: 'Navigate the Settings page',
          body: 'Use the sticky section links to jump to Appearance, General, Services, Accounts, Launching, Java, Launcher, Updates, and Privacy when Advanced mode is enabled. Reset appearance or layout if experiments become difficult to read or navigate.',
        },
      ],
    },
    advanced: {
      summary: 'Build a precise application theme and operational configuration while preserving accessibility and understanding which controls affect layout, typography, or external behavior.',
      outcomes: [
        'Use custom colors without losing contrast.',
        'Separate density, text scale, corner, and motion effects.',
        'Diagnose launcher-path and application-update settings.',
      ],
      sections: [
        {
          title: 'Construct a custom palette',
          body: 'Open Custom colors to tune block, navigation, background, and text colors plus surface opacity. Check light and dark content, selected states, warnings, disabled controls, and focus rings before keeping the palette.',
          callout: {
            tone: 'warning',
            title: 'Contrast applies to states too',
            text: 'Readable body text is not enough. Verify muted text, links, borders, destructive actions, and keyboard focus against every custom surface.',
          },
        },
        {
          title: 'Know each preference boundary',
          body: 'Density changes layout spacing and control dimensions. Text scale changes typography. Corner style overrides ordinary rounded utilities while intentional pills and circles remain round. Motion governs transitions, animation, and smooth scrolling throughout the app.',
        },
        {
          title: 'Maintain the launcher shell',
          body: 'Use Launcher Path only when auto-detection cannot find the official launcher, then test the selected executable. Software Updates checks signed release delivery through GitHub Releases and can download and restart Agora. Reset Layout restores the sidebar dimensions without touching instance data.',
        },
      ],
    },
  },
  {
    id: 'accounts-services',
    title: 'Accounts and optional services',
    shortTitle: 'Accounts & services',
    category: 'Connect',
    description: 'Understand why Agora offers GitHub, Microsoft, Modrinth, and Copilot connections.',
    keywords: ['account', 'github', 'microsoft', 'modrinth', 'copilot', 'services'],
    basic: {
      summary: 'Agora does not require every account for every feature. Connect only the service needed for the task you want to perform.',
      outcomes: [
        'Know which account enables each feature.',
        'Avoid confusing separate GitHub sign-ins.',
        'Disable optional services without losing local instances.',
      ],
      sections: [
        {
          title: 'Match services to purposes',
          body: 'Modrinth adds live catalog and download access. GitHub governance sign-in enables voting and community participation. Microsoft sign-in enables direct in-app online launching. GitHub Copilot powers the optional integrated AI Assistant.',
        },
        {
          title: 'Keep account roles separate',
          body: 'The GitHub account under Accounts is for governance. The AI Assistant may request its own GitHub Copilot authorization. Microsoft is a different identity used for Minecraft ownership and online authentication during direct launch.',
          callout: {
            tone: 'note',
            title: 'Delegated launch stays simple',
            text: 'When using the official launcher, manage the Minecraft account there. Agora does not need a Microsoft sign-in for delegated launch.',
          },
        },
        {
          title: 'Disconnect or disable',
          body: 'Turn off an integration in Services to stop using it, and use its account control to sign out where available. Existing local instances remain. Some live descriptions, catalog entries, voting, direct-launch, or AI features may no longer be available.',
        },
      ],
    },
    advanced: {
      summary: 'Treat account state, feature consent, and network egress as separate layers. A service works only when all required layers are available.',
      outcomes: [
        'Diagnose enabled-but-unavailable integrations.',
        'Understand account expiration and fallback behavior.',
        'Configure a least-privilege service set.',
      ],
      sections: [
        {
          title: 'Trace the enablement chain',
          body: 'For live Modrinth access, service consent, Modrinth API network permission, CDN permission, and the absence of global Lockdown must align. Similar chains apply to registry updates, governance, runtime downloads, and authentication.',
        },
        {
          title: 'Diagnose account state',
          body: 'If governance or AI stops working, check whether the correct GitHub connection is active and whether its session expired. If direct launch cannot authenticate, check Microsoft account status separately from Java and loader health.',
        },
        {
          title: 'Use least privilege',
          body: 'Disable services you do not use, keep direct-launch credentials unnecessary when delegated launch meets your needs, and enable network endpoints only for planned workflows. Review Privacy after onboarding because feature toggles do not replace backend network policy.',
          callout: {
            tone: 'tip',
            title: 'Separate diagnosis by layer',
            text: 'Check feature toggle, account state, endpoint permission, global Lockdown, and connectivity in that order.',
          },
        },
      ],
    },
  },
  {
    id: 'privacy-offline',
    title: 'Privacy, networking, and offline use',
    shortTitle: 'Privacy & offline',
    category: 'Customize',
    description: 'See every network category, disable egress, and prepare Agora for offline operation.',
    keywords: ['privacy', 'network', 'lockdown', 'offline', 'telemetry', 'endpoints'],
    basic: {
      summary: 'Agora makes no automated telemetry calls. The Privacy section documents functional network destinations and lets Advanced-mode users disable them individually or all at once.',
      outcomes: [
        'Understand why Agora contacts each service category.',
        'Use Lockdown mode intentionally.',
        'Prepare cached data before going offline.',
      ],
      sections: [
        {
          title: 'What network access is for',
          body: 'Network access can support Modrinth discovery and files, GitHub registry updates and governance, Mojang metadata and content, loader downloads, Microsoft authentication, Java runtime downloads, application updates, and optional AI services.',
        },
        {
          title: 'Use Lockdown mode',
          body: 'Enable Advanced mode, open Privacy, and turn on Lockdown to block all external network calls. Local instances, cached catalog data, installed content, snapshots, and other offline operations remain available.',
          callout: {
            tone: 'warning',
            title: 'Online launch may need the network',
            text: 'Authentication, missing game files, loaders, runtimes, or mods cannot be fetched while Lockdown is active.',
          },
        },
        {
          title: 'Prepare for offline play',
          steps: [
            'Update the Agora registry.',
            'Launch each required instance once so game and loader files are present.',
            'Confirm the selected Java runtime is installed.',
            'Download planned mods and packs before disconnecting.',
            'Enable Lockdown and test the exact launch workflow while you still have time to correct missing files.',
          ],
          body: 'Offline readiness is instance-specific. One prepared instance does not guarantee that another has all required artifacts.',
        },
      ],
    },
    advanced: {
      summary: 'Build a network policy per capability and use the live status and endpoint inventory to audit Agora\'s functional egress.',
      outcomes: [
        'Map features to endpoint permissions.',
        'Apply dependency-aware network restrictions.',
        'Distinguish UI consent from backend enforcement.',
      ],
      sections: [
        {
          title: 'Apply capability-based policy',
          body: 'Enable only the endpoint groups needed for the workflow: registry, governance, mod discovery, launch, runtime, authentication, updates, or AI. Global Lockdown overrides individual choices.',
          bullets: [
            'Disabling the Modrinth API also prevents the associated CDN workflow.',
            'Runtime access is separate from Mojang game content and loader content.',
            'A cached registry does not imply that every referenced artifact is cached.',
          ],
        },
        {
          title: 'Audit degraded behavior',
          body: 'When a feature fails, compare its service consent with its endpoint permission and the live Online/Offline indicator. The UI preference is persisted, while the Rust backend is responsible for enforcing requests.',
        },
        {
          title: 'Protect shared diagnostics',
          body: 'Logs can contain local paths, usernames, server addresses, or chat context. Lockfiles intentionally omit private configuration contents, but you should still review sources and instance names before sharing. AI analysis sends selected context to its provider.',
          callout: {
            tone: 'note',
            title: 'No telemetry does not mean no network',
            text: 'Agora avoids automated analytics. User-requested catalog, download, authentication, update, governance, and AI features still require functional requests when enabled.',
          },
        },
      ],
    },
  },
  {
    id: 'governance',
    title: 'Community governance and curation',
    shortTitle: 'Community governance',
    category: 'Connect',
    description: 'Understand curated entries, triage polls, reviews, resolutions, and the transparency log.',
    keywords: ['governance', 'curation', 'vote', 'triage', 'reviews', 'transparency'],
    basic: {
      summary: 'Agora\'s catalog is community curated. Governance makes inclusion, review, and removal activity visible instead of hiding those decisions behind a private service.',
      outcomes: [
        'Read active polls and completed resolutions.',
        'Vote or flag responsibly through GitHub.',
        'Understand what the Curated label does and does not promise.',
      ],
      sections: [
        {
          title: 'Read the governance page',
          body: 'Active Triage Polls show projects currently under review. Recent Resolutions records completed outcomes. The Transparency Log lists governance actions and timestamps so catalog decisions remain inspectable.',
        },
        {
          title: 'Participate with GitHub',
          body: 'Connect GitHub under Accounts to see live poll data and follow vote links. Read the linked evidence and community rules before voting. Flag a review only for a specific policy or safety concern, not because you dislike a project.',
        },
        {
          title: 'Interpret curation',
          body: 'Curated means an entry passed Agora\'s community process and includes reviewed metadata. It does not guarantee that every version works with every other mod or that a project can never develop a security issue.',
          callout: {
            tone: 'tip',
            title: 'Check the Agora tab',
            text: 'A curated detail page can include curator notes, categories, community reviews, status, and an immunity explanation.',
          },
        },
      ],
    },
    advanced: {
      summary: 'Use governance signals as auditable community evidence while keeping technical compatibility, moderation outcomes, and project trust as distinct questions.',
      outcomes: [
        'Interpret vote distributions and action records critically.',
        'Understand immunity and triage without overgeneralizing them.',
        'Contribute actionable evidence to the registry process.',
      ],
      sections: [
        {
          title: 'Separate governance signals',
          body: 'Net score, vote velocity, reviews, triage status, immunity, and compatibility are different dimensions. A strong community score is not an artifact hash check; a technical incompatibility is not automatically a governance violation.',
        },
        {
          title: 'Read resolutions in context',
          body: 'Resolution badges identify the action category, while the transparency entry records when and why it occurred. Follow the source discussion for evidence and scope. A decision about one release or behavior may not generalize to every historical version.',
        },
        {
          title: 'Submit useful evidence',
          body: 'Provide exact project and artifact versions, reproducible behavior, logs where appropriate, source links, and the policy criterion involved. Remove personal data and avoid speculative accusations.',
          callout: {
            tone: 'warning',
            title: 'Governance is not a support shortcut',
            text: 'Use project support or Crash Doctor for ordinary compatibility bugs. Use governance for curation, conduct, integrity, or community-policy concerns.',
          },
        },
      ],
    },
  },
  {
    id: 'ai-assistant',
    title: 'Integrated AI Assistant',
    shortTitle: 'AI Assistant',
    category: 'Connect',
    description: 'Use optional Copilot chat for explanations while controlling the context you share.',
    keywords: ['ai', 'assistant', 'copilot', 'chat', 'crash explanation'],
    basic: {
      summary: 'The optional AI Assistant can explain crash information, suggest troubleshooting steps, and answer Agora or modding questions. It should support, not replace, backups and verified compatibility data.',
      outcomes: [
        'Connect and use the integrated assistant.',
        'Ask focused questions with useful context.',
        'Recognize privacy and accuracy limits.',
      ],
      sections: [
        {
          title: 'Enable and connect',
          body: 'Turn on Integrated AI Assistant in Settings, open its sidebar destination, and connect with GitHub when prompted. This Copilot connection can be separate from the GitHub account used for governance.',
        },
        {
          title: 'Ask better questions',
          body: 'State the Minecraft version, loader, exact mod versions, what changed, what you expected, and the first relevant error. Ask for a small diagnostic sequence rather than a long list of unrelated fixes.',
          steps: [
            'Describe the goal and observed failure.',
            'Include exact versions and the relevant log excerpt.',
            'Ask the assistant to separate evidence from guesses.',
            'Apply one reversible step at a time.',
            'Verify the result in Agora and Minecraft.',
          ],
        },
        {
          title: 'Protect your data',
          body: 'Assistant messages and attached crash context are sent to GitHub Copilot. Review logs for personal paths, usernames, server addresses, tokens, or private chat before submitting them.',
          callout: {
            tone: 'warning',
            title: 'AI can be wrong',
            text: 'Do not download unknown files, weaken security controls, or delete saves only because an AI response suggests it. Prefer Agora\'s reviewed plan and recovery tools.',
          },
        },
      ],
    },
    advanced: {
      summary: 'Use the assistant as a hypothesis generator with controlled context, deterministic evidence, and explicit verification.',
      outcomes: [
        'Structure crash context for higher-quality analysis.',
        'Manage provider limits and connection state.',
        'Audit suggested actions before execution.',
      ],
      sections: [
        {
          title: 'Use structured context',
          body: 'When opened from Crash Doctor, the assistant can receive the crash log, matched signatures, and ranked suspects. Ask it to cite the exact evidence for each hypothesis and to preserve Crash Doctor\'s one-variable testing discipline.',
        },
        {
          title: 'Work within service limits',
          body: 'The UI reports the connected state and provider rate limits. If the assistant is unavailable, deterministic Crash Doctor, console filters, snapshots, and lockfiles still work. AI is optional and should never be the only recovery path.',
        },
        {
          title: 'Review every proposed operation',
          body: 'Translate suggestions into Agora actions: snapshot, resolve an install plan, disable with dependent review, verify a lockfile, or inspect Java. Reject commands that bypass hash verification, expose credentials, or modify unrelated files.',
          callout: {
            tone: 'tip',
            title: 'Request falsifiable tests',
            text: 'A useful answer predicts what evidence should change if the hypothesis is correct. Test that prediction before accepting the diagnosis.',
          },
        },
      ],
    },
  },
  {
    id: 'mcp-automation',
    title: 'MCP and external AI tools',
    shortTitle: 'MCP & automation',
    category: 'Connect',
    description: 'Connect an external local AI client and control what it may do to instances.',
    keywords: ['mcp', 'external ai', 'automation', 'localhost', 'bearer token', 'tools'],
    basic: {
      summary: 'MCP is an advanced optional bridge that lets a compatible AI tool inspect Agora and request supported actions. Most users do not need it; the integrated AI Assistant is simpler for occasional help.',
      outcomes: [
        'Decide whether MCP is appropriate for your workflow.',
        'Start and stop the local server.',
        'Understand the security impact before connecting a client.',
      ],
      sections: [
        {
          title: 'Choose integrated AI or MCP',
          body: 'Use Integrated AI for chat inside Agora. Use MCP when you already operate an external AI client and want it to list instances, analyze crashes, or request supported management actions through Agora\'s tool interface.',
        },
        {
          title: 'Connect a client',
          body: 'Enable Advanced mode and AI / MCP Server in Settings. Start the server, use the displayed local server URL and configuration instructions for your client, and provide the generated Bearer token where the client supports authorization.',
          steps: [
            'Start the MCP server only when needed.',
            'Copy the configuration for Kilo Code, Opencode, Claude Desktop, or another compatible client.',
            'Store the Bearer token like a local credential.',
            'Confirm the client can list instances before allowing any modifying action.',
            'Stop the server when the workflow is complete.',
          ],
        },
        {
          title: 'Approve changes carefully',
          body: 'An AI request is not proof that an action is safe. Review the target instance and requested operation. Keep important instances locked and use snapshots before permitting changes.',
          callout: {
            tone: 'warning',
            title: 'Local does not mean harmless',
            text: 'The server listens on localhost, but other processes running as you may reach local services. Protect the token and regenerate it if exposed.',
          },
        },
      ],
    },
    advanced: {
      summary: 'Operate MCP as a privileged local automation surface: authenticate clients, scope approvals per instance, monitor state, and preserve transactional recovery.',
      outcomes: [
        'Configure client authentication and rotate credentials.',
        'Apply least privilege to modifying tools.',
        'Troubleshoot connectivity without weakening the local boundary.',
      ],
      sections: [
        {
          title: 'Secure the connection',
          body: 'Use the displayed 127.0.0.1 SSE endpoint, never expose it through port forwarding or a public reverse proxy, and include the Bearer token through the client\'s supported authorization method. Regeneration invalidates the previous token and requires updating every client.',
        },
        {
          title: 'Scope tool approvals',
          body: 'Use per-instance approval settings for modifying tools such as disable_mod. Default to confirmation or denial for valuable instances. Read-only inspection is lower risk but can still expose instance names, file metadata, or crash context to the connected AI provider.',
        },
        {
          title: 'Troubleshoot methodically',
          body: 'Check that the server status is running, the URL uses 127.0.0.1 and port 39741, the client transport is SSE, the token is current, and local security software is not blocking the process. Do not disable system security or bind the server broadly to solve a client configuration error.',
          callout: {
            tone: 'note',
            title: 'Preserve Agora\'s safety rails',
            text: 'Prefer MCP tools that route modifications through Agora\'s existing dependency, snapshot, and health workflows rather than direct filesystem commands.',
          },
        },
      ],
    },
  },
];

export const GUIDE_CATEGORIES: GuideTopic['category'][] = [
  'Start',
  'Play',
  'Manage',
  'Recover',
  'Customize',
  'Connect',
];
