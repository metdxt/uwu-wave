# Sample library — sources and licenses

The audio in this folder comes from several free sources; the first line of every `zones.txt`
names its source. Keep this file with the library.

## Virtual Playing Orchestra (DecentSampler edition)
<https://github.com/eodowd/VirtualPlayingOrchestra>, compiled by Paul Battersby
(<http://virtualplaying.com/virtual-playing-orchestra/>). Used for: the string sections
(`violins-1`, `violins-2`, `violas`, `cellos`, `basses`, `strings-ensemble`), `violin-solo`,
`viola-solo`, `brass/{horns,trombones,trumpet,tuba,brass-ensemble}`,
`woodwinds/{flutes,oboes,clarinets,bassoons,piccolo,woodwinds-ensemble}`, `choir/*`,
`plucked/harp`, `percussion/{celesta,glockenspiel,xylophone,vibraphone,tubular-bells,timpani}` and
the impulse responses in `ir/`. It combines:

- Sonatina Symphonic Orchestra <http://sso.mattiaswestlund.net/> — Creative Commons Sampling Plus 1.0.
- Mattias Westlund additional samples <http://mattiaswestlund.net/samples/> — CC BY-SA 3.0 Unported.
- No Budget Orchestra <https://github.com/ssj71/No-Budget-Orchestra> — CC BY-SA 4.0.
- VSCO 2 Community Edition <http://vis.versilstudios.net/vsco-2.html> — CC0 1.0.
- University of Iowa Electronic Music Studios <http://theremin.music.uiowa.edu/> — "these
  recordings have been freely available on this website and may be downloaded and used for any
  projects, without restrictions".
- stamperadam <https://freesound.org/people/stamperadam/> — CC0.
- A few Philharmonia Orchestra samples, modified by the VPO author (see its README).

The authors of these sources are aware of their use in Virtual Playing Orchestra (see its README).

## Philharmonia Orchestra sound samples — NOT bundled
<https://philharmonia.co.uk/resources/sound-samples/>. Their licence says the samples "must not be
sold or made available 'as is' (i.e. as samples or as a sampler instrument)", so this library does
not contain them. Download the official archives yourself and run
`uwu library import philharmonia`. The converted instruments (solo violin, viola, cello, double
bass, woodwinds, horn, trumpet, trombone, tuba, guitar, mandolin, banjo and percussion) go to the
git-ignored overlay `assets/local/`, which takes precedence over this folder. Never commit or share
that folder.

## Piano (`keys/piano`)
The 88-key piano recordings from the original `assets/piano` folder. Check the original source
before redistributing the raw samples.

## cluesurf/wave (`plucked/{bass-guitar,electric-guitar,electric-guitar-fly,celtic-harp,zither}`, `drums/*`, `sfx/*`)
<https://github.com/cluesurf/wave> — "100% public domain, free sound samples". These zones are
*remote*: `zones.txt` has `url=`, `size=` and `sha256=`, and uwu-wave downloads each file the
first time it's used into `~/.cache/uwu-wave/lfs` (`UWU_OFFLINE=1` disables downloading).
