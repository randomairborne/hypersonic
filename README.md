# hypersonic

hypersonic is a discord music bot written in [rust](https://rust-lang.org).

hypersonic is designed to play music files on loop in one VC.

create a music folder, with your files and a file called `meta.json`. That file should look something like this:

```json
[
    {
        "name": "11",
        "artist": "C418",
        "album": "Minecraft: Volume Beta",
        "file": "11.mp3"
    },
    {
        "name": "13",
        "artist": "C418",
        "album": "Minecraft: Volume Beta",
        "file": "13.wav"
    }
]
```

This example would load `11.mp3` and `13.wav` from the music folder, and play them.

Then, create a .env file (or otherwise set the environment):

```dotenv
DISCORD_VC=<your VC's ID>
DISCORD_GUILD=<the ID of the guild DISCORD_VC is in>
DISCORD_TOKEN=<your bot's token>
```

You can use this command to run the bot in docker:

```bash
docker run ghcr.io/randomairborne/hypersonic:latest --env-file .env --volume ./music/:/music/
```