<div align="center">

# 🤫 Hushnote

### Meeting notes that never leave your computer.

Hit record. Talk. Walk away with clean notes, decisions and action items,
without sending a single word of your meeting to someone else's cloud.

**Free · Open source · 100% private**

</div>

---

## Your meetings are confidential. Your notes tool should be too.

Most AI note-takers work the same way: your audio goes up to a server, gets
processed somewhere you can't see, and comes back as a summary. Your client
calls, your hiring chats, your roadmap debates: all of it stored on a company's
servers.

**Hushnote does it all on your own machine.** The recording, the transcript and
the summary never touch the internet. No account. No subscription. No "we may use
your data to improve our models."

## What you get

🎙️ **Just press record**
Hushnote listens to your mic and to the people on the call, so you get both
sides of the conversation.

📝 **Live transcript**
Watch the conversation turn into text as it happens. No more "wait, what did
they just say?"

✍️ **Write your own notes, we'll do the rest**
Jot down what matters to you while the meeting runs. Afterwards, Hushnote
combines your notes with the transcript into a clear summary: key points,
decisions and who has to do what.

🔒 **Private by design**
Everything runs locally. Works on a plane, in a basement, or under an NDA.

🌍 **Speaks your language**
Understands 25 languages, including English, Spanish, French, German,
Portuguese and more.

📂 **Your notes are just files**
Every meeting is saved as a plain Markdown file in your Documents folder. Open
it in any editor, sync it with the tool you already use, keep it forever. No
lock-in, no export button needed.

⚡ **Fast on your hardware**
Uses your graphics card when available, and still runs fine without one.

## Who is it for?

- **Founders and freelancers** who talk to clients all day and can't afford to
  lose a single detail.
- **Managers** tired of writing recaps after every one-on-one.
- **Consultants, lawyers, therapists, journalists** and anyone whose
  conversations are simply not allowed to leave the room.
- **Anyone** who wants to be fully present in a meeting instead of typing
  frantically.

## Why Hushnote?

| | Typical cloud note-takers | **Hushnote** |
|---|---|---|
| Your audio is sent to a server | Yes | **Never** |
| Monthly subscription | Usually | **Free** |
| Works offline | No | **Yes** |
| Account required | Yes | **No** |
| Notes stored in | Their app | **Plain files you own** |
| Source code | Closed | **Open (MIT)** |

## Get started

Hushnote is in its **first early version** and is being built for **Linux**
(with [Omarchy](https://omarchy.org) as the main target).

1. Grab the code: `git clone https://github.com/SebastianV75/hushnote`
2. Build it with Rust: `cargo build --release`
3. Press record on your next meeting.

Installers and a simple one-click setup are on the way.

## What's coming

- A polished one-click installer
- Telling speakers apart ("who said what")
- Search across all your past meetings
- A tray icon and a global shortcut to start recording instantly
- Importing audio files you already have

Have an idea? [Open an issue](https://github.com/SebastianV75/hushnote/issues),
we'd love to hear it.

## Spread the word

If Hushnote sounds like something you or your team would use, a ⭐ on this repo
is the best way to help it reach more people.

## Built with

[Rune](https://github.com/anomalyco/rune) for the interface and
[transcribe-cpp](https://crates.io/crates/transcribe-cpp), the same local
speech engine behind [Hex](https://github.com/anomalyco/hex).

## License

MIT. Free for personal and commercial use.
