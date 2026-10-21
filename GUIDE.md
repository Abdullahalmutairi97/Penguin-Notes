# Penguin Notes: the easy guide

This guide is for everyone. You don't need to know anything about computers beyond clicking, copying and pasting. Go step by step, and skip any part you don't want (only Part 1 is needed).

**What Penguin Notes does**

- 📝 A notes app for your computer. Everything you write is saved by itself.
- 🐧 **Pengu**, a helper you can chat with about your notes. It can also write and tidy them.
- 🎙️ **Talk**: speak to Pengu instead of typing. Your voice stays on your computer.
- 📱 **iPhone sync**: your notes show up in the iPhone's **Notes** app, and your to-dos in its **Calendar** app.

---

## Part 1: Install the app

### On Windows

1. Open this page: **https://github.com/Abdullahalmutairi97/Penguin-Notes/releases/latest**
2. Scroll down to **Assets** and click **PenguinNotes-Setup.exe**. It downloads.
3. Open the downloaded file (it's usually in your **Downloads** folder).
4. Windows may show a blue box saying **"Windows protected your PC"**. That's only because the app is new and not "signed". Click **More info**, then **Run anyway**.
5. Click **Next** until it says **Finish**. Leave **"Open Penguin Notes"** ticked.

The app is now in your **Start menu** and on your desktop as **Penguin Notes**.

### On Linux (Ubuntu, Mint, Fedora, …)

1. Open the **Terminal** app (on Ubuntu, press **Ctrl + Alt + T**).
2. Copy this whole line, paste it into the Terminal (**Ctrl + Shift + V**) and press **Enter**:

   ```
   curl -fsSL https://github.com/Abdullahalmutairi97/Penguin-Notes/releases/latest/download/install.sh | sh
   ```

3. Wait until it says **"Penguin Notes is installed."** You can close the Terminal.
4. Open **Penguin Notes** from your apps menu, like any other app.

> If it says `curl: command not found`, first paste `sudo apt install curl`, press Enter, type your computer password, then try step 2 again.

---

## Part 2: Writing notes

- Click **+ New** (top left) to make a note. The first line is its title.
- Everything saves by itself. There is no Save button.
- Use the buttons above the page for **bold**, *italic*, lists and checkboxes.
- **Edit / Split / Preview** (at the top) switch between writing, writing with a preview beside it, and just reading.
- Type `#something` (for example `#work`) to tag a note. Search tags in the search box on the left.
- Click **Calendar** at the top to see your days and to-dos.

Handy keys: **Ctrl + N** new note · **Ctrl + F** search · **Ctrl + B** bold · **Ctrl + I** italic.

---

## Part 3: Set up Pengu (the helper)

Pengu needs a "brain" (an AI model). The easiest choice is a paid online one, which costs a few cents a month for normal use. Or use a free one that runs on your own computer.

### Easy way: an online AI (recommended)

1. Make an account with **DeepSeek** (platform.deepseek.com) or **OpenAI** (platform.openai.com). Add a little credit, say $5.
2. On their site, find **API keys** and click **Create new key**. Copy the long code it shows you (it starts with `sk-`). Treat it like a password.
3. In Penguin Notes, click **Pengu** (top right), then **Settings**.
4. Click **DeepSeek** or **OpenAI** (whichever you signed up for).
5. Paste your code into **API key**.
6. Click **Pick a model** and choose one. Then click **Test connection**. It should say it works.
7. Close the settings. Type a question to Pengu, like *"Make me a shopping list for pasta night"*.

### Free way: on your own computer

1. Install **LM Studio** from lmstudio.ai, open it, and download a model it suggests.
2. In LM Studio, turn on the **local server**.
3. In Penguin Notes: **Pengu > Settings > LM Studio > Pick a model**.

(This needs a fairly strong computer. If it's slow, use the online way.)

---

## Part 4: Talk to Pengu with your voice

1. Open Pengu (top right) and click **Talk**.
2. **The very first time only**, the button says **"Getting voice ready… 0%"**. The app is downloading its speech engine (about 190 MB, like a short video). Wait until it reaches 100% and the button says **Talk** again.
3. Click **Talk**, say what you want, then click **Done** to send it.
4. The **EN / AR** button next to it picks English or Arabic.

Your voice is turned into text **on your computer**. Nothing is sent to the internet.

Shortcut: **Ctrl + Shift + Space** to start and stop talking.

> **Linux:** if it says it "can't record", paste `sudo apt install alsa-utils` in the Terminal and press Enter.

---

## Part 5: Your to-dos in the iPhone Calendar

Your day pages' to-dos appear in a **"Penguin Notes"** calendar on your iPhone. Events you add on the phone come back too.

**Step A: make a special password at Apple** (this keeps your real password safe)

1. On any computer or phone, go to **account.apple.com** and sign in.
2. Go to **Sign-In and Security**, then **App-Specific Passwords**.
3. Click **+** (or **Generate**), name it **Penguin Notes** and click **Create**.
4. Apple shows a code like `abcd-efgh-ijkl-mnop`. **Copy it.**

**Step B: connect in Penguin Notes**

1. Click **More** (top right), then **iPhone sync…**
2. Under **Calendar**, type your **Apple ID email** and paste the code into **App-specific password**.
3. Click **Connect & sync**. After a few seconds it says **"Synced"** with the time.

On your iPhone, open **Calendar** and tap **Calendars** at the bottom. You'll see **Penguin Notes**. It updates every few minutes.

---

## Part 6: Your notes in the iPhone Notes app

This works through an email account the iPhone already knows (Gmail is the most common). The iPhone keeps notes inside that email account. Penguin Notes puts your notes there too, so they appear on the phone, and changes you make on the phone come back.

Below are the steps for **Gmail**. Yahoo and Fastmail work almost the same: the app shows the right link for each.

**Step A: make an app password at Google**

1. Go to **myaccount.google.com/apppasswords** and sign in.
   - If Google says app passwords aren't available, first turn on **2-Step Verification** (myaccount.google.com > **Security**), then come back.
2. Type the name **Penguin Notes** and click **Create**.
3. Google shows 16 letters in a yellow box. **Copy them.**

**Step B: connect in Penguin Notes**

1. Click **More > iPhone sync…** and scroll to **Apple Notes**.
2. Click **Gmail**.
3. Type your **Gmail address** in **Email** and paste the 16 letters in **App password**.
4. Click **Connect & sync**. After a few seconds it says **"Synced"**.

**Step C: turn on Notes on the iPhone**

1. Open **Settings > Apps > Mail > Mail Accounts**.
2. If **Gmail** is in the list, tap it. If not, tap **Add Account > Google**, sign in, then tap it.
3. Turn on the **Notes** switch.
4. Open the **Notes** app and go back to the list of folders. Under **GMAIL** there is a **Notes** folder with your Penguin Notes in it.

It updates about every minute. Notes show as plain text on the iPhone. The formatting marks (like `**bold**`) stay as they are.

---

## Updating to a new version

- **Windows:** download the newest **PenguinNotes-Setup.exe** from the same page and run it. Your notes stay.
- **Linux:** run the same one line from Part 1 again. Your notes stay.

## Removing the app

- **Windows:** **Settings > Apps**, find **Penguin Notes**, click **Uninstall**.
- **Linux:** paste this in the Terminal and press Enter:
  `sh ~/.local/share/penguin-notes-app/install.sh --uninstall`

Your notes are kept either way, in case you come back.

---

## Something isn't working?

| What you see | What to do |
|---|---|
| "Windows protected your PC" | Click **More info**, then **Run anyway**. |
| Pengu says to check your API key | The code was copied wrong or has no credit left. Paste it again in **Pengu > Settings**. |
| "Pick a model in Pengu settings first" | Do Part 3. |
| "user name or password was rejected" (iPhone sync) | Use the **app password** from Step A, not your normal password. |
| "can't reach …" | Check that your internet works, then click **Sync now**. |
| Voice download stopped | Check your internet and click **Talk** again. It starts over. |
| Notes don't show on the iPhone | Check that the **Notes** switch is on for that account (Part 6, Step C). |

**Where are my notes kept?** On your own computer:
Windows `%APPDATA%\penguin-notes`, Linux `~/.local/share/penguin-notes`.
