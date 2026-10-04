#!/usr/bin/env bash
# Build the chukcut showcase: generate demo media with ffmpeg, then build a
# project with chukcut-cli that uses most of the editor's features.
#
#   scripts/demo.sh            # media (if missing) + project
#   scripts/demo.sh --media    # regenerate the media, then the project
#   scripts/demo.sh --export   # also export _scratch/demo/showcase.mp4
#   scripts/demo.sh --no-ml    # leave out the steps that need ML models
#
# Everything goes to _scratch/demo/ (ignored by git). Media never enters git.
# The CLI runs with its own HOME and XDG folders under _scratch/demo/xdg, so
# the script does not change your settings, presets or recent files.
# docs/demo.md explains the result.
#
# The ML steps (optical-flow slow motion, select object, remove object,
# enhance quality, isolate voice) use the models and the GPU runtime you
# already installed (`chukcut-cli ml install gpu`): the script links your ML
# folder (~/.cache/chukcut/ml, or $CHUKCUT_DEMO_ML_CACHE) into its own cache
# and downloads nothing. Without them it uses frame blending instead of
# optical flow, skips the other ML steps and says so.
set -euo pipefail

root=$(CDPATH="" cd -- "$(dirname "$0")/.." && pwd)
cd "$root"

cli=${CHUKCUT_CLI:-$root/target/release/chukcut-cli}
out=$root/_scratch/demo
media=$out/media
p=$out/showcase.chukcut

regen_media=0
do_export=0
use_ml=1
for arg in "$@"; do
  case $arg in
    --media) regen_media=1 ;;
    --export) do_export=1 ;;
    --no-ml) use_ml=0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

[[ -x $cli ]] || { echo "no CLI at $cli; run: cargo build --release -p chukcut-cli" >&2; exit 1; }

# The ML models and runtimes the user already has, found before HOME moves.
user_ml=${CHUKCUT_DEMO_ML_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/chukcut/ml}

# Isolated settings: presets, the LUT library and the working copy stay here.
export HOME=$out/xdg/home
export XDG_CONFIG_HOME=$out/xdg/config
export XDG_CACHE_HOME=$out/xdg/cache
export XDG_DATA_HOME=$out/xdg/data
mkdir -p "$media" "$HOME" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME/chukcut" "$XDG_DATA_HOME"
# Read-only use of the user's models: mattes and flow frames are baked into
# the isolated cache, never into the user's.
if [[ $use_ml == 1 && -d $user_ml && ! -e $XDG_CACHE_HOME/chukcut/ml ]]; then
  ln -s "$user_ml" "$XDG_CACHE_HOME/chukcut/ml"
fi

ff() { ffmpeg -hide_banner -loglevel error -y "$@"; }

# ---------------------------------------------------------------------------
# 1. Media
# ---------------------------------------------------------------------------
make_media() {
  echo "== generating media in $media"
  local W=1080 H=1920 R=30

  # A quiet room tone, so each clip has a sound stream (linked audio lanes).
  local tone="sine=frequency=110:sample_rate=48000"

  # Moving colour gradients: the opening shot.
  ff -f lavfi -i "gradients=s=${W}x${H}:r=$R:d=6:speed=0.03:n=5:seed=7" \
     -f lavfi -i "$tone:duration=6" -filter:a "volume=0.05" \
     -c:v libx264 -preset veryfast -pix_fmt yuv420p -c:a aac -shortest "$media/01-gradients.mp4"

  # A Mandelbrot zoom, rendered small and scaled up (the filter is slow).
  ff -f lavfi -i "mandelbrot=s=540x960:rate=$R:start_scale=3:end_scale=0.05:end_pts=180" \
     -f lavfi -i "$tone:duration=6" -filter:a "volume=0.05" \
     -vf "scale=$W:$H:flags=bicubic" -t 6 \
     -c:v libx264 -preset veryfast -pix_fmt yuv420p -c:a aac -shortest "$media/02-fractal.mp4"

  # The test pattern with a hue sweep: good for the speed ramp, it shows time.
  ff -f lavfi -i "testsrc2=s=${W}x${H}:r=$R:d=6" \
     -f lavfi -i "$tone:duration=6" -filter:a "volume=0.05" \
     -vf "hue=h=t*60" \
     -c:v libx264 -preset veryfast -pix_fmt yuv420p -c:a aac -shortest "$media/03-pattern.mp4"

  # A red ball that moves over a textured background: the tracking target.
  # The texture gives the tracker features; the ball path is a figure of
  # eight, about 20 px per frame at most.
  ff -f lavfi -i "testsrc2=s=${W}x${H}:r=$R:d=6" \
     -f lavfi -i "color=c=red:s=180x180:r=$R:d=6,format=rgba,geq=r='220':g='30':b='30':a='if(lte(hypot(X-90,Y-90),88),255,0)'" \
     -f lavfi -i "$tone:duration=6" \
     -filter_complex "[0:v]eq=brightness=-0.25:saturation=0.4[bg];[bg][1:v]overlay=x='450+330*sin(t*1.2)':y='870+520*sin(t*2.4)'[v]" \
     -map "[v]" -map 2:a -filter:a "volume=0.05" \
     -c:v libx264 -preset veryfast -pix_fmt yuv420p -c:a aac -shortest "$media/04-ball.mp4"

  # A rotating test card on a green screen: the chroma-key clip.
  ff -f lavfi -i "testsrc=s=600x600:r=$R:d=5" \
     -vf "rotate=a=t*0.8:c=0x00b140:ow=900:oh=900,pad=$W:$H:(ow-iw)/2:(oh-ih)/2:color=0x00b140" \
     -an -c:v libx264 -preset veryfast -pix_fmt yuv420p "$media/05-greenscreen.mp4"

  # Conway's life in colour: the picture-in-picture clip.
  ff -f lavfi -i "life=s=270x480:rate=$R:mold=10:ratio=0.12:life_color=#ffb000:death_color=#5020a0:mold_color=#101030" \
     -t 5 -vf "scale=$W:$H:flags=neighbor" \
     -an -c:v libx264 -preset veryfast -pix_fmt yuv420p "$media/06-life.mp4"

  # A comet: a bright disc that crosses moving gradients fast, about 17 px
  # a frame. Slowed to half speed, optical flow makes the missing frames.
  ff -f lavfi -i "gradients=s=${W}x${H}:r=$R:d=3:speed=0.02:n=4:seed=3" \
     -f lavfi -i "color=c=black:s=220x220:r=$R:d=3,format=rgba,geq=r='255':g='235':b='150':a='255*max(0,min(1,(105-hypot(X-110,Y-110))/12))'" \
     -f lavfi -i "$tone:duration=3" \
     -filter_complex "[0:v][1:v]overlay=x='-220+t*500':y='1500-t*420'[v]" \
     -map "[v]" -map 2:a -filter:a "volume=0.05" \
     -c:v libx264 -preset veryfast -pix_fmt yuv420p -c:a aac -shortest "$media/07-comet.mp4"

  # A small, blocky clip with a white "watermark" box in the top right: the
  # remove-object and enhance-quality clip. 270x480 at a high CRF, so 4x
  # enhance brings it to the canvas size and shows what the model cleans.
  ff -f lavfi -i "mandelbrot=s=270x480:rate=$R:start_scale=0.4:end_scale=0.1:end_pts=60:inner=convergence" \
     -f lavfi -i "$tone:duration=2" -filter:a "volume=0.05" \
     -vf "drawbox=x=180:y=20:w=70:h=36:color=white@1:t=fill" -t 2 \
     -c:v libx264 -preset veryfast -crf 38 -pix_fmt yuv420p -c:a aac -shortest "$media/08-lowres.mp4"

  # A voiceover from flite (offline speech synthesis in FFmpeg).
  local words="Welcome to chukcut. A native video editor for Linux. \
Cut, grade and track your clips. Add titles, captions and stickers. \
Then export straight to TikTok."
  ff -f lavfi -i "flite=text='$words':voice=slt" \
     -af "aresample=48000,apad=pad_dur=0.5,loudnorm=I=-16" -ac 1 -ar 48000 "$media/voice.wav"

  # A music bed: a kick on every beat at 120 BPM over a minor chord.
  ff -f lavfi -i "aevalsrc=exprs='0.55*sin(2*PI*55*t*(1+2*exp(-mod(t,0.5)*30)))*exp(-mod(t,0.5)*9)+0.06*(sin(2*PI*220*t)+sin(2*PI*261.63*t)+sin(2*PI*329.63*t))*(0.6+0.4*sin(2*PI*0.25*t))':s=48000:d=30" \
     -af "afade=t=out:st=28:d=2" -ac 2 "$media/music.wav"

  # A sticker: a yellow smiley with a transparent background.
  ff -f lavfi -i "color=c=black:s=512x512:d=1,format=rgba" -frames:v 1 \
     -vf "geq=r='255':g='196':b='0':a='if(lte(hypot(X-256,Y-256),240)*not(lte(hypot(X-180,Y-200),34))*not(lte(hypot(X-332,Y-200),34))*not(between(hypot(X-256,Y-270),120,150)*gt(Y,290)),255,0)'" \
     "$media/sticker.png"

  # An animated sticker: a cyan ring that pulses, with a transparent
  # background. The palette keeps one colour for "transparent"; without
  # palettegen's reserve_transparent the GIF comes out opaque.
  ff -f lavfi -i "color=c=black@0:s=256x256:r=15:d=2,format=rgba,geq=r='40':g='210':b='255':a='255*between(hypot(X-128,Y-128),40+40*abs(sin(T*3.1416)),70+50*abs(sin(T*3.1416)))',split[a][b];[a]palettegen=reserve_transparent=1[pal];[b][pal]paletteuse=alpha_threshold=128" \
     -loop 0 "$media/pulse.gif"

  # A teal-and-orange LUT (17 points per axis): shadows to teal, highlights
  # to orange, a little more contrast.
  python3 - "$media/teal-orange.cube" <<'PY'
import sys
n = 17
with open(sys.argv[1], "w") as f:
    f.write('TITLE "teal orange (demo)"\nLUT_3D_SIZE %d\n' % n)
    for b in range(n):
        for g in range(n):
            for r in range(n):
                rgb = [r / (n - 1), g / (n - 1), b / (n - 1)]
                luma = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
                # S-curve for contrast.
                rgb = [c + 0.15 * (c - 0.5) * (1 - abs(2 * c - 1)) for c in rgb]
                shadow, light = (1 - luma) ** 2, luma ** 2
                rgb[0] += 0.10 * light - 0.06 * shadow
                rgb[1] += 0.02 * light + 0.02 * shadow
                rgb[2] += -0.10 * light + 0.08 * shadow
                f.write("%.6f %.6f %.6f\n" % tuple(min(1, max(0, c)) for c in rgb))
PY
}

# Captions for the voiceover, in timeline time: voice.wav goes on the
# timeline at 0.5 s (see below), and these cues are flite's sentences
# (measured with silencedetect) shifted by that 0.5 s.
cat > "$media/voice.srt" <<'SRT'
1
00:00:00,700 --> 00:00:01,850
Welcome to chukcut.

2
00:00:01,950 --> 00:00:03,700
A native video editor for Linux.

3
00:00:03,850 --> 00:00:05,700
Cut, grade and track your clips.

4
00:00:05,800 --> 00:00:07,950
Add titles, captions and stickers.

5
00:00:08,100 --> 00:00:09,850
Then export straight to TikTok.
SRT

if [[ $regen_media == 1 || ! -f $media/teal-orange.cube || ! -f $media/07-comet.mp4 || ! -f $media/pulse.gif || ! -f $media/08-lowres.mp4 ]]; then
  make_media
fi

# ---------------------------------------------------------------------------
# 2. The project, built only with chukcut-cli
# ---------------------------------------------------------------------------
echo "== building $p"

# Run a CLI command; print its one-line result.
c() { "$cli" "$@"; }
# Run a CLI command with --json and print one field of its data (a jq path).
cj() { local path=$1; shift; "$cli" --json "$@" | jq -r ".data$path"; }

# Whether the ML steps can run here without a download: a GPU runtime and
# the model files. On the CPU they work but take minutes, so the showcase
# leaves them out there too.
ml_ready() {
  [[ $use_ml == 1 ]] || return 1
  local runtime models
  runtime=$("$cli" --json ml status | jq -r '.data.runtime_id // ""')
  [[ $runtime == cuda* ]] || return 1
  models=$("$cli" --json ml models | jq -r '[.data[] | select(.downloaded) | .id] | join(" ")')
  local m
  for m in "$@"; do
    [[ " $models " == *" $m "* ]] || return 1
  done
}
skipped=()

c new "$p" --name "chukcut showcase" --width 1080 --height 1920 --fps 30 --force

# The main story on lane 0, and the rest of the media in the library.
c import "$p" "$media/01-gradients.mp4" "$media/02-fractal.mp4" \
  "$media/03-pattern.mp4" "$media/04-ball.mp4" "$media/07-comet.mp4" \
  "$media/08-lowres.mp4" --append
c import "$p" "$media/05-greenscreen.mp4" "$media/06-life.mp4" \
  "$media/voice.wav" "$media/music.wav" "$media/sticker.png"

grad=$(cj '.tracks[0].clips[0].id' info "$p")
frac=$(cj '.tracks[0].clips[1].id' info "$p")
patt=$(cj '.tracks[0].clips[2].id' info "$p")
ball=$(cj '.tracks[0].clips[3].id' info "$p")
comet=$(cj '.tracks[0].clips[4].id' info "$p")
low=$(cj '.tracks[0].clips[5].id' info "$p")

# Cut: 4 s, 4.5 s, 3 s of source for the ramp, then the ball clip.
c trim "$p" "$grad" --duration 4 --ripple
c trim "$p" "$frac" --duration 4.5 --ripple
c trim "$p" "$patt" --duration 3 --ripple

# A speed effect: a ramp and the frame smoothing that suits it, one step.
# Bullet time is fast, a slow-motion hold in the middle, fast again, with
# optical flow; without the model, Smooth montage (frame blending).
if ml_ready rife; then
  c speed-effect "$p" "$patt" --effect bullet
else
  c speed-effect "$p" "$patt" --effect montage
  skipped+=("Bullet time on the pattern (Smooth montage instead)")
fi

# Smooth slow motion on the comet: half speed, and RIFE makes the frames in
# between (optical flow). Without the model and a GPU runtime, frame
# blending stands in.
if ml_ready rife; then
  c smooth-slow-mo "$p" "$comet" --speed 0.5
else
  c set "$p" "$comet" --speed 0.5
  c frame-blend "$p" "$comet" --mode blend
  skipped+=("optical flow on the comet (frame blending instead)")
fi

# Transitions on the three cuts: a gl-transition, a seamless one, a dissolve.
c transition add "$p" "$frac" --kind gl:crosswarp --duration 0.8
c transition add "$p" "$patt" --kind seamless:zoom_in --duration 0.6
c transition add "$p" "$ball" --kind dissolve --duration 0.5

# The small clip at the end: the watermark box removed (LaMa paints over a
# box, the same place in every frame), then enhanced 4x (Real-ESRGAN) from
# 270x480 to 1080x1920. The object is removed first, then the result is
# enhanced.
if ml_ready lama; then
  c remove-object "$p" "$low" --box 0.65,0.03,0.3,0.1
else
  skipped+=("remove object (the watermark box) on the last clip")
fi
if ml_ready realesr-general-x4v3; then
  c enhance-quality "$p" "$low" --scale 4
else
  skipped+=("enhance quality 4x on the last clip")
fi

end=$(cj '.duration' info "$p")

# ---- Sound: a voiceover, a music bed that ducks under it, captions.
voice=$(cj '.clip.id' place "$p" voice.wav --at 0.5)
music=$(cj '.clip.id' place "$p" music.wav --at 0 --duration "$end")
c normalize "$p" --clip "$voice" --target -14
c audio-effect add "$p" "$voice" eq3 --set low=-3 --set high=2
# Isolate voice (HTDemucs) keeps the speech and drops the room tone.
if ml_ready htdemucs-vocals; then
  c isolate-voice "$p" "$voice" --keep voice
else
  skipped+=("isolate voice on the voiceover")
fi
c duck "$p" "$music" --depth 10
c audio-effect add "$p" "$music" reverb

# Captions from the SRT, with the karaoke look: the spoken word lights up
# (word times are estimated per cue on import).
c captions import "$p" "$media/voice.srt" --preset karaoke
c captions style "$p" --position -0.55 --highlight "#ffd400"

# ---- Look: grade + LUT + a tone curve on the fractal; effects.
c grade "$p" "$frac" --set exposure=0.1 --set contrast=1.15 --set saturation=1.25 \
  --set temperature=0.15 --set vignette_amount=0.35 \
  --lut "$media/teal-orange.cube" --lut-intensity 0.8
c curve "$p" "$frac" --point 0,0 --point 0.25,0.2 --point 0.75,0.82 --point 1,1
# Auto adjust balances the opener from its own pixels; colour match gives
# the comet the fractal's teal-and-orange look (no model, a few seconds each).
c auto-adjust "$p" "$grad" --amount 0.6
c colour-match "$p" "$comet" --to "$frac"
c effect add "$p" glow --clip "$grad" --set intensity=45 --set threshold=75
c effect add "$p" film_grain --clip "$frac" --set amount=25
# A punch-in zoom on the fractal, eased over its first second.
c zoom "$p" "$frac" --amount 1.15 --duration 1 --easing ease_out
# A shake effect clip on an effect lane, over the cut into the ball clip.
ball_start=$(cj '.tracks[0].clips[3].start' info "$p")
c effect add "$p" shake --at "$(echo "$ball_start - 0.2" | bc)" --duration 0.6 --set amplitude=45

# ---- Titles: a template (style + animation), then a styled title animated
# word by word, one after the other.
head=$(cj '.clip.id' title template "$p" pop-headline --at 0.2 --text "chukcut")
c trim "$p" "$head" --duration 1.9
c title set "$p" "$head" --y 0.45
sub=$(cj '.clip.id' title add "$p" "native video editing on Linux" --at 2.1 --duration 1.9 \
  --size 64 --color "#ffffff" --bold true --stroke-width 6 --stroke-color "#101020" --y 0.25)
c animate-text "$p" "$sub" --preset fade_up --unit word --duration 1.2
c animate "$p" "$sub" --slot out --preset fade --duration 0.4

# ---- A sticker: an image with a transparent background, scaled, animated
# in with a pop and kept moving with a loop, plus rotation keyframes.
stk=$(cj '.clip.id' place "$p" sticker.png --at 6 --duration 3)
c set "$p" "$stk" --scale 0.3 --x 0.55 --y 0.55
c animate "$p" "$stk" --slot in --preset pop --duration 0.5 --easing back
c animate "$p" "$stk" --slot combo --preset wobble --duration 1
c keyframe "$p" "$stk" --property rotation --at 6 --value -10
c keyframe "$p" "$stk" --property rotation --at 9 --value 15 --easing ease_in_out

# ---- Picture in picture: Conway's life in a rounded frame, top left,
# blended with Screen.
pip=$(cj '.clip.id' place "$p" 06-life.mp4 --at 4.5 --duration 3.5)
c layout pip "$p" "$pip" --corner top_left
# Crop it to a square from the middle: the cropped part fills the frame.
c crop "$p" "$pip" --left 0.05 --right 0.95 --top 0.24 --bottom 0.76
c blend "$p" "$pip" screen --opacity 0.95

# ---- Green screen: key the green out, then a rounded rectangle mask as a
# garbage matte that opens up over a second.
gs=$(cj '.clip.id' place "$p" 05-greenscreen.mp4 --at 9 --duration 4.2)
c chroma-key "$p" "$gs" --color "#00b140" --tolerance 0.3 --softness 0.1 --spill 0.6
c set "$p" "$gs" --scale 0.8 --y 0.15
c mask "$p" "$gs" --add rectangle --set width=0.2 --set height=0.2 --set roundness=0.4 \
  --set feather=0.03 --at 9
c mask "$p" "$gs" --mask 0 --set width=0.9 --set height=0.9 --at 10

# ---- Tracking: a label follows the red ball. The box is drawn on the
# ball's first frame, where the ball sits at the centre of the frame.
ball_start=$(cj '.tracks[0].clips[3].start' info "$p")
ball_end=$(cj '.tracks[0].clips[3].end' info "$p")
tag=$(cj '.clip.id' title add "$p" "tracked" --at "$ball_start" \
  --duration "$(echo "$ball_end - $ball_start" | bc)" --size 72 --color "#ffffff" --bold true \
  --background "#e0202080" --y 0.12)
c track "$p" "$ball" --at "$ball_start" --rect 0.5,0.5,0.17,0.095 --overlay "$tag" --mode position

# ---- Select object: MobileSAM picks the ball on one frame, VitTrack
# carries it over the clip. The grade then applies to the background only
# (saturation 0, darker), so the red ball keeps its colour. The matte stays
# uncut: remove-background --off keeps the background in the picture.
if ml_ready mobilesam mobilesam-decoder vittrack; then
  # One second into the clip the ball's centre is at (847, 1313) px.
  c select-object "$p" "$ball" --at "$(echo "$ball_start + 1" | bc)" --point 0.785,0.684
  c apply-to "$p" "$ball" --grade background
  c grade "$p" "$ball" --set saturation=0 --set exposure=-0.3
  c remove-background "$p" "$ball" --off
else
  skipped+=("select object + background-only grade on the ball")
fi

# ---- An animated sticker over the comet: a pulsing GIF that crosses the
# frame fast, with motion blur on its move.
comet_start=$(cj '.tracks[0].clips[4].start' info "$p")
pulse=$(cj '.clip.id' sticker "$p" --file "$media/pulse.gif" --at "$(echo "$comet_start + 0.5" | bc)")
c set "$p" "$pulse" --scale 0.6 --y 0.35
c keyframe "$p" "$pulse" --property x --at "$(echo "$comet_start + 0.5" | bc)" --value -0.8
c keyframe "$p" "$pulse" --property x --at "$(echo "$comet_start + 1.5" | bc)" --value 0.8 --easing ease_in_out
c keyframe "$p" "$pulse" --property x --at "$(echo "$comet_start + 2.5" | bc)" --value 0 --easing ease_out
c effect add "$p" motion_blur --clip "$pulse" --set shutter=300 --set samples=12

# ---- A compound clip: the two intro titles become one clip, "Intro titles",
# that can be moved, animated or opened (double-click in the app) as one.
c compound create "$p" "$head" "$sub" --name "Intro titles"

# ---- Markers on the ruler at each section.
c marker add "$p" --at 0 --label "Intro" --color green
c marker add "$p" --at 4 --label "Grade + LUT" --color purple
c marker add "$p" --at 8.5 --label "Speed ramp" --color orange
c marker add "$p" --at "$ball_start" --label "Tracking" --color red
c marker add "$p" --at "$comet_start" --label "Slow motion" --color blue
c marker add "$p" --at "$(cj '.tracks[0].clips[5].start' info "$p")" --label "Remove + enhance" --color yellow

# ---- A second timeline, "Template cut", made from the Quick Cuts template:
# six slots cut to the beat with its own titles, transitions and music, put
# into the showcase as a timeline of its own (one undo step) and opened. Its
# clips stay editable there, and `template slots` lists its slots.
c template apply --into "$p" quick-cuts "$media/01-gradients.mp4" "$media/02-fractal.mp4" \
  "$media/03-pattern.mp4" "$media/04-ball.mp4" "$media/05-greenscreen.mp4" \
  "$media/06-life.mp4" --as timeline --name "Template cut"
# A label of our own over the template's titles, on a text lane of its own.
c lane-add "$p" --kind text --name "Label"
c title add "$p" "made from a template" --at 0.2 --duration 2.5 --size 56 \
  --color "#ffffff" --bold true --background "#00000099" --y -0.7 --track "Label"
# Back to the main timeline: the app opens on it and the export renders it.
c timeline switch "$p" 0

# ---- Delivery: a preset of our own on top of the TikTok preset. Presets
# live in the (isolated) config folder, not in the project.
c preset save "$p" "Showcase" --preset tiktok --crf 19
c estimate "$p" --preset user_showcase

c validate "$p"
c info "$p"
c timeline list "$p"
for note in "${skipped[@]}"; do
  echo "note: ML step left out (--no-ml, or no GPU runtime and models): $note" >&2
done

if [[ $do_export == 1 ]]; then
  c export "$p" "$out/showcase.mp4" --preset user_showcase --sidecar srt
  ffprobe -v error -show_entries stream=codec_name,width,height,r_frame_rate,nb_frames:format=duration \
    -of compact "$out/showcase.mp4"
fi
