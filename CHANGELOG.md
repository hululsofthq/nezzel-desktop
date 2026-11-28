# Changelog

## 0.1.7

- The Nezzel browser takes your browser's sign-in on the first visit of a known sign-in site (YouTube, Facebook…), so the page opens signed in instead of waiting for the site to ask. The switch, private tabs and the per-site setting still apply.
- A video whose site gives it a very long id (some news articles) no longer fails with "File name too long" while saving its preview picture.

## 0.1.6

- Catch from the page: when yt-dlp can't read a link, Nezzel opens the page in the background, catches the video it plays and offers every quality. If the page needs a click, a sign-in or a captcha, the row says so and opens the page.
- The Nezzel browser: one window with tabs in a side list (folds to icons), a home page with one search/address field, recent sites and recently saved videos, and a "Video found" row with Save on any page that plays a video. Pop-ups you didn't ask for are blocked; one you clicked opens as a linked tab.
- The Nezzel browser blocks ads and trackers, with a count of what it blocked; "Block ads" turns it off for one site if a page stops working.
- Search engine: DuckDuckGo by default; Google, Bing or Brave Search from the field's switcher or Settings.
- Your browser's sign-in is used by default for the site you open (only that site's sign-in, never in private tabs, can be switched off per site). "Mobile version" reloads a page as a phone sees it.
- Before saying a video can't be saved, Nezzel tries the mobile version and yt-dlp; only a video the site really encrypts is left. "Why?" on that row explains it and offers "Report this site".
- Every link opened from the main window goes to a tab in the same Nezzel browser window.
- Settings lists near the bottom of the window open upward instead of past the window's edge.
- Files a page downloads in the Nezzel browser (a PDF, a ZIP, an installer) are saved through your download list, in the Files folder, instead of your default browser. A file Nezzel can't save says so; private tabs and tabs behind the proxy still refuse.
- Cmd-click or middle-click on a link in the Nezzel browser opens it in a background tab. A page that can't load shows "Couldn't open" with "Try again".
- Catch from the page plays videos inside embedded players, presses play past cookie bars that don't cover the video, and offers the clip that actually plays, not a longer video loaded beside it.
- Copying several links adds each one. With "Ask how to save each link" off, a link the clipboard watch picks up starts downloading by itself.
- After Nezzel restarts, interrupted downloads stay in their playlist, links waiting under "Needs you" come back with "Reload link", and an interrupted "Download again" keeps its own row. Return in an empty link field (spaces only) says "Paste a link first".
- The download bar and the download row offer the same next step for a link Nezzel can't read directly.
- After a download that needed a sign-in finished without one, the row no longer names the browser profile used before.

## 0.1.5

- Instagram: every file of a post with videos and photos is named the same way, after the post's folder and its place in the post ("Post by name-001", "-002", …), and the row shows how many items the post has before you open it, while it downloads and after Nezzel restarts (posts saved with an earlier 0.1.5 test build are corrected at the next start).
- Instagram: a post that was saved completely no longer shows "Showing the items found before the site stopped responding".
- Instagram: when a post with videos and photos is saved without a sign-in, the video Nezzel can get is kept in the post's folder. On Windows it was saved in a folder at the top of the drive instead of your download folder (the row said "The other items were saved" while the folder looked empty); on macOS and Linux it was dropped.
- An album or photo post you already saved now says "Already saved" and offers "Download again", which saves a second copy beside the first.
- Reddit: when Reddit refuses downloads from your network, the message says so and offers to open the post on Reddit, instead of asking you to sign in.
- With a proxy set in Settings, Nezzel's own connections (checking for and downloading updates, error reports, usage counts) go through the proxy too.
- What's new: Settings → About now lists what changed in each version, in your language. The update card shows the new version and what it brings, with "Skip this version", and after an update one line in the sidebar points to the list.
- The portable Windows copy now tells you when a new version is out and opens the download page; before, it never said anything.
- Several links at once: a link that cannot be read no longer stops the others from downloading; a message names the one that failed.
- A download that could not be saved now says why (the disk is full, there is no permission, the name is too long) instead of one English message, and files already saved are announced. Photo saves report their real cause too.
- Reddit: a post with only text, one that only links to another site, or a Reddit app post (a game or tool that runs inside Reddit) says so and offers to open the post. More messages that say "Open…" now have the open button.
- Filters: a download skipped by a saved filter names the filter (date, size or advanced); the link field's menu button shows a dot and the download options show a line while a filter is on. A download that saved nothing with no filter set is no longer described as filtered.
- Sign-in: a sign-in that failed names the cause (the browser took too long, its cookies could not be read, the sign-in window was closed), and a second link for the same site reuses the sign-in the first one made; if you close the window instead, the next link opens its own (on Windows it used to fail silently).
- A failed video inside a post with photos keeps its real cause (rate limit, disk full, network), and a download that fails offline or with an expired session says so, as reading the link already did.
- Updates: Nezzel checks again every 12 hours while it stays open, and once more when you return to it after a failed check. An update download with no progress for 60 seconds stops with "Retry", and the card has "Cancel". On Linux (deb and rpm) the card says the system may ask for your password; a refused prompt gets its own message, and "Retry" does not download the package again.
- Portable Windows copy: 0.1.3 and 0.1.4 cannot tell you about this version; download 0.1.5 once by hand and it will tell you about the next ones.
- Files saved by direct download get the normal file permissions on macOS and Linux, so other accounts and media servers can read them.
- A custom folder layout can no longer place files outside the download folder.
- Leftover partial files from a crash are removed at the next start; a failed tool download leaves nothing behind.
- Long lists: the finished list shows 200 rows with "Show more", and actions on many rows no longer run all at once.
- Arabic: numbers with nouns use the correct form for every count.
- Arabic on macOS: Settings no longer scrolls under the window buttons.
- A post that can only be saved as photos has a "Download the photos" button.
- A link to a site that cannot be found says "Couldn't find this site" instead of "No internet connection" while you are online.
- An expired browser sign-in says so and asks you to sign in again.
- Arrow keys move past the first 200 finished downloads and into a playlist's folded items.
- The "could not check free disk space" and "download stopped making progress" messages no longer show two periods.

## 0.1.4

- A link that gives two or more files (a playlist, an album, a post with several videos or photos) is now saved in one folder named after it. To keep everything in one place as before, choose "No subfolders" under "Organise downloads by" in Settings; if you had already chosen it, choose it once more.
- Several links pasted or submitted together start downloading by themselves instead of waiting under "Needs you".
- The items of a playlist or of a post with several videos each keep their own preview picture in the list, and the saved videos listed under a finished download show a preview like saved photos do.
- With a proxy set in Settings, every page Nezzel reads goes through the proxy; before, some built-in page readers connected directly.
- A large photo album keeps downloading while photos are still arriving instead of stopping at the time limit.
- Instagram: a post with videos and photos is saved in one folder, and it keeps the videos it already saved when the photos need a sign-in.
- Facebook: a "share/p" link to a photo post is saved after you sign in.
- Tumblr: saved photos get readable file names, and when Tumblr refuses the page the message says so instead of asking you to wait.

## 0.1.3

- A post with several videos (for example an X post with ten clips) saves each video; before, the first video was saved once and copied under the other names.
- Reddit: posts with several photos, links ending in "#lightbox" and profile posts are saved. When Reddit is limiting requests from your network, such a post can still end with a message asking you to open it in the browser; trying again later works. A post that has nothing to save says so.
- Reddit: when Reddit asks for a login, Nezzel now tries your browser's Reddit login before giving up, and the "blocked" message no longer says that signing in cannot help.
- Picking videos from a playlist while a download filter (date, size) excludes them now says "Skipped by your link filters" with a button to edit the filters, instead of "This page has no videos to save".

## 0.1.2

- Windows: updating from inside the app no longer stops at a language question, which could open behind other windows and leave the update waiting. The setup now follows your system language.
- The copied-link card closes a few seconds after you leave it; on the first link after starting Nezzel it could stay for about a minute. It also no longer keeps an empty area after its formats list closes by itself.
- Resuming or removing a file download right after cancelling it no longer fails with "This file is already downloading".
- macOS: sites that Nezzel must contact the way a browser does work again (9gag failed with an unknown error).
- An Instagram photo post that needs a login now says "sign in" instead of "no media found".
- Bluesky profiles: videos are saved as .mp4 files in their original quality, and saving only a part of a video works.
- Saving only a part of a video: when the site answers "not found" or a server error, the message says so instead of a generic failure.
- Bluesky posts saved at best quality keep the original upload (1080p where the post has it) instead of the 720p copy.
- Photo albums: a photo the site refuses with "too many requests" is tried once more after a short wait, and if it still fails the message says so instead of "the other items were saved" (seen on large Flickr albums).
- A cancelled download keeps its picture in the list after a restart.

## 0.1.1

- Windows: a first download that Windows blocked while it was still checking the new file ("Access is denied") is now retried once by itself; if it still cannot be moved, the message says so instead of a plain "Couldn't download".
- Windows: the first link you read after installing no longer ends in "taking too long to answer"; the first read gets more time and the built-in tools are started once in the background.
- Windows: smaller once installed (the video tools share their libraries instead of shipping them twice), and the installer lists Hululsoft as the publisher.
- A link you already saved now says "Already saved. Nothing new was downloaded", and the row menu has "Download again" when you want a second copy.
- A cancelled download stays in the list as Cancelled after a restart, with Retry, and is never resumed by itself.
- Clicking the toolbar right after copying a new link adds the new link, not the previous one.
- Photo albums: each saved photo shows its own small preview, opens when you click its picture or name, and Arabic titles read correctly next to the photo number.
- Flickr albums and YouTube pages that answer "too many requests" now say so instead of reporting success or an unknown error.
- On a system with no Downloads folder, files go to Downloads/Nezzel in your home folder.
- Signing in: when one browser's login does not work, Nezzel tries the next one, with a single sign-in window.
- The welcome screen's sample card shows the real clip's quality and size.

## 0.1.0

First stable release.

- Automatic login: when a site needs you signed in, Nezzel uses your browser's login with no questions asked. The first time, a short note says so; you can turn it off in Settings > Logins. If you are not signed in anywhere, Nezzel can open a sign-in window for that site. Only that site's cookies are used, and they never leave your computer.
- Many more link shapes work: TikTok short links and photo posts, Bilibili player links, Facebook share/photo/m./web. links, Threads share links, and links copied with invisible characters around them. Expired links say "removed" and unsupported sites say so plainly instead of a generic error.
- New file hosts: pixeldrain, Yandex Disk, MediaFire, Internet Archive and GitHub (release files, raw and blob links).
- yt-dlp now updates without a new Nezzel release, from a signed list we publish.
- Linux: copied links are picked up on Wayland too (via wl-paste); the menu shows the right category; tray menu follows the app language.
- Cancel really stops a download and removes its leftover partial files; paused downloads keep theirs.
- Library keeps each download's details and failed rows across restarts; the card stays open while your mouse is on it; Instagram reels show their caption as the title.
- Windows: first YouTube link works without a retry, Facebook share links retry once, language switch updates every menu.
- Fixed many smaller issues found in the beta.4 test round on macOS, Windows and Linux.

## 0.1.0-beta.4

- Facebook photo posts (single photos, albums shared with "share/p", photo.php links) now save the photos instead of stopping at "this post has photos, not a video".
- Threads: a slow answer from Threads no longer turns into "not supported"; Nezzel retries once and follows threads.com/share links to the post.
- Vimeo: a video that needs a login now says so instead of a generic "couldn't download".
- Copied-link card: faster on YouTube, shows the post's caption as the title and its picture, hides the size column when sizes are unknown, and its button no longer looks broken while loading.
- Library: Video, Audio, Photos and Files show only your files; failed downloads stay in All downloads, with one line pointing to them. Remove all failed from the list, or report several failed downloads at once with one note.
- A failed download's page shows the message once and a Copy details button; the raw log is no longer on screen, and copied details keep one line per entry.
- macOS: the first time you close the window while Nezzel keeps running, a note says where it went and how to reopen it.
- Photo file names no longer contain "pNA".
- Anonymous usage counts (when on) now include the failure code, so we can see which errors are common.

## 0.1.0-beta.3

- Playlists: every way of saving a playlist opens a checklist first (select all or none, Space, Ctrl/Cmd+A, one quality for all, "Download N videos"); nothing starts until you choose. The options sheet no longer closes on its own.
- New "Needs you" section for downloads waiting on your choice or a login.
- Logins: one choice, Ask / Never / Always, plus the browser to use; cookies.txt stays under Advanced. With a fixed browser, cookies are read only for sites that need a login.
- New About page in Settings with Report a problem, version and update check. The window opens 880 px wide so the sidebar shows its labels.
- Paste several links at once; after dropping links, "Download all"; unsupported sites offer "Open in browser" and no pointless Retry.
- Clearer tool repair: the real cause of a failed tool download, an offline notice, and an honest "continue without tools".
- Arabic: sizes and speeds in Latin MB and MB/s, natural "time left", login labels read right to left; on macOS the window buttons no longer cover the Continue button or the sheet close button.
- Fixed: the app could crash on quit while reading the clipboard (beta.1); a crash in a background task is now reported at the next start (when reports are on).
- Privacy text now says plainly that crash reports and anonymous usage counts are on by default and can be turned off.

## 0.1.0-beta.2

Reliability release.

- Fixed: the browser login you chose was never passed to the download tool, so sign-in-only posts (Instagram, Reddit and others) failed even when you were signed in.
- Fixed: clips, live streams and some stream formats failed on macOS and Linux because the bundled ffmpeg could not verify HTTPS certificates.
- Photos from Flickr and Tumblr save again; X posts with a video and photos save everything; Facebook single-photo links save the photo.
- Vimeo links that answered "401" are retried once the way a browser would ask.
- Clearer messages: content your account cannot see, a browser whose cookies cannot be read (Keychain, Full Disk Access, browser open), disk full, file name too long, protected (DRM) streams, Facebook links the site does not hand out, Reddit posts with no media.
- Long non-Latin titles no longer produce file names that are too long on Linux.
- A quiet notice when the download tools could not update for a week, with "Try now".
- Failures the app cannot classify, and timeouts while online, are now included in error reports (same scrubbing and switch as before).

## 0.1.0-beta.1

First public beta of Nezzel Desktop, a fork of Open Video Downloader 3.2.1.

- Paste or copy a link and Nezzel reads it at once: videos, audio, photos, carousels and plain files.
- One engine picks the right tool per site (yt-dlp, gallery-dl, built-in readers for Reddit, Threads, Instagram and others) and falls back on its own when one fails.
- Faster: slideshows download in one batch; X and Reddit videos start without a second lookup.
- Sites that need an account use the browser you're already signed in to, only after you say yes, and only that site's cookies are passed on.
- Clear failure messages (sign-in needed, region blocked, removed, rate limited) instead of "unknown error".
- Library keeps your download history; "Remove from list" never deletes files.
- English, Arabic (right-to-left) and French.
- Crash and error reports to our own Sentry project (EU), on by default with a switch in Settings and on the welcome screen; no links, file names, paths or cookies are sent automatically. No analytics.

Known limits: macOS and Windows builds are not signed yet (see the install note in the README).
