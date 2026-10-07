# What parterre sends

parterre sends three things, each with its own switch in *Settings → Privacy*: the
[update check](#update-check), [usage statistics](#usage-statistics) and
[crash reports](#crash-reports). It also asks GitHub for [pull requests](#pull-requests) when
you are signed in with `gh`.

**Nothing is sent before the first-run prompt is answered.** At first start, parterre asks in a
dialog, *Usage statistics and crash reports*, with *Usage statistics* ticked and *Crash
reports* unticked. It closes only with *Continue*.

## Update check

At start, and then every 24 hours while parterre is open, parterre asks GitHub's releases API
whether a newer release is out. It sends nothing of parterre's own: no install ID, no version,
nothing about you or your repositories, and its user agent is just `parterre`. GitHub sees the
IP address, as with any request; its
[privacy statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement)
covers the request.

When a newer release is out, a dialog says so the first time parterre hears of it, once per
version: *parterre 0.8.0 is out*, a link to its release notes, and *Download*. Until you
update, *Download 0.8.0* opens the *Help* menu on Windows and Linux, whose title turns blue,
and sits under *About parterre* on macOS, where parterre's Dock icon gets a badge.

On by default. *Check for updates* in *Settings → Privacy* turns it off; then nothing is asked.
Snap and Flatpak never check, because their stores update parterre, and neither does a build
without the [`send` feature](building.md#features). There *Check for updates* is off and greyed
out.

## Usage statistics

On unless you untick them. Sent to PostHog as parterre is used, with the
[install ID](#install-id). They never contain personal information.

| Event | When |
|---|---|
| `Application Installed` | the first start |
| `Application Updated` | the first start of a new version, with the previous version |
| `Application Opened` | every start |
| `Application Backgrounded` | closing parterre |
| `$screen` | a window or dialog opens: its name, such as `log`, `diff`, `settings`, `merge` or `reset` |
| `menu_view` | a menu opens: which one, such as the File menu, a node's context menu or the zoom popover |
| `action_run` | you start something: what, such as `merge`, `create_branch`, `reload`, `export`, `fit` or `find`; not whether it succeeded |

Every event carries:

- parterre's name, version and channel (MSI, zip, tarball, `.deb`, `.rpm`, Snap, cargo and so
  on), and the version of git it runs
- the operating system and its version
- the language and region setting, and the time zone
- the screen size and the display's scale
- your settings for theme, text size, graph mode and direction, log layout, dragging, diff
  form, automatic reload and pull requests
- how many repositories your recent list holds (at most 10), and roughly how big the open
  repository is: its commits and the graph's nodes, as a range such as 1000–9999
- a random session ID, which changes after 30 minutes without use, after 24 hours at most, and
  whenever you open another repository; it says nothing about which repository

Feature and setting names come from a fixed list built into parterre; the rest are numbers and
yes/no answers. None of it includes anything from your repositories or anything you type into
parterre.

Unticking *Usage statistics* stops all of it, the install ID included.

## Install ID

A random identifier created the first time parterre runs and never changed, so that each
installation is counted once. It goes with the usage statistics. It is never tied to anything
personal: PostHog keeps the events without a person profile. It is never sent with a crash
report.

*Settings → Privacy* shows it, with *Copy*, for [requests about your data](#your-rights).

## Crash reports

Off unless you tick them. When parterre panics, PostHog's SDK sends a report at that moment:

- the panic message, and where in parterre's code it happened
- the stack trace: function names, source file paths and addresses
- the executable and system libraries loaded, with their paths and build IDs, so that the
  stack trace can be read
- parterre's version and channel, the version of git it runs, the operating system and its
  version, the language and region setting, and the time zone

Paths in your home folder become `~`. A report may still contain personal information,
such as a file or branch name in the panic message.

Each report has a random ID of its own, never the install ID or the session ID. There is no
local copy, and nothing is asked at the next start. Crash reports have their own switch: they
are sent even with *Usage statistics* unticked.

Ticking *Crash reports* in the first-run prompt takes effect at once; a change in *Settings →
Privacy* takes effect from the next start. Only panics are reported: aborts,
stack overflows and crashes in native code, such as a graphics driver, aren't caught.

## Never sent

parterre never puts any of these in usage statistics, crash reports or the update check:

- repository paths and names
- remote URLs
- branch, tag and commit data
- file names and contents
- git user names and emails
- environment variables
- hashes of any of these

A panic message can still name a file or a branch. That is why crash reports are off unless
ticked.

## Pull requests

When `origin` is on GitHub and `gh` is signed in, parterre asks GitHub's API, with `gh`'s token,
for the open pull requests of the branches you have fetched. That sends the repository's owner
and name, and those branch names, to GitHub, which hosts them. Nothing is asked without signing
in. The toolbar's pull-request button turns it off ([user guide](usage.md#pull-requests)).

Before you delete a branch on a remote that is on GitHub, parterre asks once more, with
pull requests shown or not, whether an open pull request proposes that branch: it won't delete
one that does. That sends the branch names being deleted.

## Who is responsible

Trustfall AB, Yachtvägen 35, 749 48 Enköping, Sweden, is the controller of the usage
statistics and crash reports under the GDPR. Contact:
[parterre@trustfall.se](mailto:parterre@trustfall.se).

## Why, and on what legal basis

- **Update check:** Trustfall AB receives nothing from it. It is a request to GitHub, under
  GitHub's privacy statement.
- **Usage statistics:** legitimate interest (GDPR art. 6(1)(f)): knowing which versions,
  platforms and features are used, so as to maintain parterre.
- **Crash reports:** consent (art. 6(1)(a)), to find and fix what makes parterre crash. Ticking
  *Crash reports* gives it, and unticking withdraws it from the next start. Reports already
  sent stay.

None of it is required: parterre works the same with all three off. Nothing is used for
automated decisions.

## Your right to object

You can object to usage statistics at any time: untick *Usage statistics* in *Settings →
Privacy*, and nothing more is sent. To have what was sent deleted too, see
[your rights](#your-rights).

## Where it goes, and for how long

- Usage statistics and crash reports go to PostHog Cloud EU, in Frankfurt, into one project in
  the maintainer's account. PostHog processes them for Trustfall AB as its processor, under
  its [data processing agreement](https://posthog.com/dpa).
- That agreement lets PostHog process data outside the EEA, in the US and elsewhere as its
  [subprocessor list](https://posthog.com/subprocessors) shows, under the EU–US Data Privacy
  Framework and the EU's standard contractual clauses.
- PostHog looks up the approximate location (country, city and the like) from the IP address,
  keeps that, and discards the address.
- PostHog's free plan keeps them for 1 year, and may delete them after that.

## Your rights

You have the GDPR's rights, where they apply: access, rectification, erasure, restriction,
objection, portability, and withdrawing consent. To use them, copy the install ID from
*Settings → Privacy* and send it to [parterre@trustfall.se](mailto:parterre@trustfall.se); it
is how your installation's usage statistics are found. Crash reports can't be tied to a person
or an install ID, so they can't be found for you.

You can also complain to Integritetsskyddsmyndigheten (IMY), Sweden's data protection
authority, at [imy.se](https://www.imy.se/en/individuals/forms-and-e-services/file-a-gdpr-complaint/),
or to the authority where you live or work.

## Turning it off

- **In parterre:** *Settings → Privacy* has *Check for updates*, *Usage statistics* and *Crash
  reports*.
- **`DO_NOT_TRACK=1`** in the environment turns usage statistics and crash reports off,
  whatever the settings say; they show as off and greyed out, and the first-run prompt isn't
  shown. The update check keeps its own switch.
- **Packagers:** build without the `send` cargo feature, which is on by default. Built that
  way, parterre makes no update check and sends nothing to PostHog
  ([building](building.md#features)).
