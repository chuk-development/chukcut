# gl-transitions in chukcut

The files in `gl/` are WGSL translations of transitions from
[gl-transitions](https://github.com/gl-transitions/gl-transitions), npm
package version 1.71.0, made by `port.py` in this directory. Each file keeps
its original header: the author, the licence, and any copyright notice and
origin note the transition carried.

123 of the upstream 125 transitions are MIT-licensed, one is BSD 3-Clause and
one BSD 2-Clause. Five are not ported:

| Transition | Why it is left out |
|---|---|
| displacement | needs a displacement map texture the renderer does not provide |
| luma | needs a luma matte texture the renderer does not provide |
| burn0 | its noise function cites a Shadertoy page as its origin (CC BY-NC-SA by default, not GPL-compatible) |
| perlin | the same Shadertoy-sourced noise function |
| tangentMotionBlur | ported upstream from a source with no stated licence |

All three licences are permissive and compatible with chukcut's GPL-3.0. The
licence texts follow the list.

## The ported transitions

| Upstream name | File | Author | Licence |
|---|---|---|---|
| AdvancedMosaic | `gl/advancedmosaic.wgsl` | Sergey Kosarevsky | MIT |
| angular | `gl/angular.wgsl` | Fernando Kuteken | MIT |
| BlockDissolve | `gl/blockdissolve.wgsl` | nwoeanhinnogaehr | MIT |
| BookFlip | `gl/bookflip.wgsl` | hong | MIT |
| Bounce | `gl/bounce.wgsl` | Adrian Purser | MIT |
| BowTieHorizontal | `gl/bowtiehorizontal.wgsl` | huynx | MIT |
| BowTieVertical | `gl/bowtievertical.wgsl` | huynx | MIT |
| BowTieWithParameter | `gl/bowtiewithparameter.wgsl` | KMojek | MIT |
| Box | `gl/box.wgsl` | lql | MIT |
| burn | `gl/burn.wgsl` | gre | MIT |
| ButterflyWaveScrawler | `gl/butterflywavescrawler.wgsl` | mandubian | MIT |
| cannabisleaf | `gl/cannabisleaf.wgsl` | @Flexi23 | MIT |
| chessboard | `gl/chessboard.wgsl` | lql | MIT |
| circle | `gl/circle.wgsl` | Fernando Kuteken | MIT |
| CircleCrop | `gl/circlecrop.wgsl` | fkuteken | MIT |
| circleopen | `gl/circleopen.wgsl` | gre | MIT |
| colorphase | `gl/colorphase.wgsl` | gre | MIT |
| ColourDistance | `gl/colourdistance.wgsl` | P-Seebauer | MIT |
| coord-from-in | `gl/coord_from_in.wgsl` | haiyoucuv | MIT |
| CrazyParametricFun | `gl/crazyparametricfun.wgsl` | mandubian | MIT |
| crosshatch | `gl/crosshatch.wgsl` | pthrasher | MIT |
| crosswarp | `gl/crosswarp.wgsl` | Eke Péter <peterekepeter@gmail.com> | MIT |
| CrossZoom | `gl/crosszoom.wgsl` | rectalogic | MIT |
| cube | `gl/cube.wgsl` | gre | MIT |
| DefocusBlur | `gl/defocusblur.wgsl` | Sergey Kosarevsky | MIT |
| Directional | `gl/directional.wgsl` | Gaëtan Renaudeau | MIT |
| directional-easing | `gl/directional_easing.wgsl` | Max Plotnikov | MIT |
| DirectionalScaled | `gl/directionalscaled.wgsl` | Thibaut Foussard | MIT |
| directionalwarp | `gl/directionalwarp.wgsl` | pschroen | MIT |
| directionalwipe | `gl/directionalwipe.wgsl` | gre | MIT |
| dissolve | `gl/dissolve.wgsl` | hjm1fb | MIT |
| DoomScreenTransition | `gl/doomscreentransition.wgsl` | Zeh Fernando | MIT |
| doorway | `gl/doorway.wgsl` | gre | MIT |
| Dreamy | `gl/dreamy.wgsl` | mikolalysenko | MIT |
| DreamyZoom | `gl/dreamyzoom.wgsl` | Zeh Fernando | MIT |
| Drop_Zone_Flicker | `gl/drop_zone_flicker.wgsl` | bread | MIT |
| EdgeTransition | `gl/edgetransition.wgsl` | Woohyun Kim | MIT |
| fade | `gl/fade.wgsl` | gre | MIT |
| fadecolor | `gl/fadecolor.wgsl` | gre | MIT |
| fadegrayscale | `gl/fadegrayscale.wgsl` | gre | MIT |
| FilmBurn | `gl/filmburn.wgsl` | Anastasia Dunbar | MIT |
| flyeye | `gl/flyeye.wgsl` | gre | MIT |
| Fold | `gl/fold.wgsl` | nwoeanhinnogaehr | MIT |
| fragment | `gl/fragment.wgsl` | lbl | MIT |
| GlitchDisplace | `gl/glitchdisplace.wgsl` | Matt DesLauriers | MIT |
| GlitchMemories | `gl/glitchmemories.wgsl` | Gunnar Roth | MIT |
| GridFlip | `gl/gridflip.wgsl` | TimDonselaar | MIT |
| heart | `gl/heart.wgsl` | gre | MIT |
| hexagonalize | `gl/hexagonalize.wgsl` | Fernando Kuteken | MIT |
| HorizontalClose | `gl/horizontalclose.wgsl` | martiniti | MIT |
| HorizontalOpen | `gl/horizontalopen.wgsl` | martiniti | MIT |
| HSVfade | `gl/hsvfade.wgsl` | nwoeanhinnogaehr | MIT |
| InvertedPageCurl | `gl/invertedpagecurl.wgsl` | Hewlett-Packard | BSD 3 Clause |
| kaleidoscope | `gl/kaleidoscope.wgsl` | nwoeanhinnogaehr | MIT |
| LeftRight | `gl/leftright.wgsl` | zhmy | MIT |
| LinearBlur | `gl/linearblur.wgsl` | gre | MIT |
| luminance_melt | `gl/luminance_melt.wgsl` | 0gust1 | MIT |
| morph | `gl/morph.wgsl` | paniq | MIT |
| Mosaic | `gl/mosaic.wgsl` | Xaychru | MIT |
| mosaic_transition | `gl/mosaic_transition.wgsl` | YueDev | MIT |
| multiply_blend | `gl/multiply_blend.wgsl` | Fernando Kuteken | MIT |
| old_tv_lost_signal | `gl/old_tv_lost_signal.wgsl` | mernking gitlab: Godswork | MIT |
| Overexposure | `gl/overexposure.wgsl` | Ben Zhang | MIT |
| parametric_glitch | `gl/parametric_glitch.wgsl` | Yoni Maltsman @friendlyspinach | MIT |
| pinwheel | `gl/pinwheel.wgsl` | Mr Speaker | MIT |
| pixelize | `gl/pixelize.wgsl` | gre | MIT |
| polar_function | `gl/polar_function.wgsl` | Fernando Kuteken | MIT |
| PolkaDotsCurtain | `gl/polkadotscurtain.wgsl` | bobylito | MIT |
| powerKaleido | `gl/powerkaleido.wgsl` | Boundless | MIT |
| PuzzleRight | `gl/puzzleright.wgsl` | JustKirillS | MIT |
| Radial | `gl/radial.wgsl` | Xaychru | MIT |
| randomNoisex | `gl/randomnoisex.wgsl` | towrabbit | MIT |
| randomsquares | `gl/randomsquares.wgsl` | gre | MIT |
| Rectangle | `gl/rectangle.wgsl` | martiniti | MIT |
| RectangleCrop | `gl/rectanglecrop.wgsl` | martiniti | MIT |
| Revolve_Left | `gl/revolve_left.wgsl` | bread | MIT |
| ripple | `gl/ripple.wgsl` | gre | MIT |
| Rolls | `gl/rolls.wgsl` | Mark Craig | MIT |
| rotate_scale_fade | `gl/rotate_scale_fade.wgsl` | Fernando Kuteken | MIT |
| RotateScaleVanish | `gl/rotatescalevanish.wgsl` | Mark Craig | MIT |
| rotateTransition | `gl/rotatetransition.wgsl` | haiyoucuv | MIT |
| scale-in | `gl/scale_in.wgsl` | haiyoucuv | MIT |
| SimpleFlip | `gl/simpleflip.wgsl` | nwoeanhinnogaehr | MIT |
| SimpleZoom | `gl/simplezoom.wgsl` | 0gust1 | MIT |
| SimpleZoomOut | `gl/simplezoomout.wgsl` | Tianshuo | MIT |
| Slides | `gl/slides.wgsl` | Mark Craig | MIT |
| splitSlideInHorizontal | `gl/splitslideinhorizontal.wgsl` | OllyOllyOlly | MIT |
| splitSlideInOutHorizontal | `gl/splitslideinouthorizontal.wgsl` | OllyOllyOlly | MIT |
| splitSlideInOutVertical | `gl/splitslideinoutvertical.wgsl` | OllyOllyOlly | MIT |
| splitSlideInVertical | `gl/splitslideinvertical.wgsl` | OllyOllyOlly | MIT |
| splitSlideOutHorizontal | `gl/splitslideouthorizontal.wgsl` | OllyOllyOlly | MIT |
| splitSlideOutVertical | `gl/splitslideoutvertical.wgsl` | OllyOllyOlly | MIT |
| squareswire | `gl/squareswire.wgsl` | gre | MIT |
| squeeze | `gl/squeeze.wgsl` | gre | MIT |
| StarWipe | `gl/starwipe.wgsl` | Ben Lucas | MIT |
| static_wipe | `gl/static_wipe.wgsl` | Ben Lucas | MIT |
| StaticFade | `gl/staticfade.wgsl` | Ben Lucas | MIT |
| StereoViewer | `gl/stereoviewer.wgsl` | Ted Schundler | BSD 2 Clause |
| StripDatamoshGlitch | `gl/stripdatamoshglitch.wgsl` | bread | MIT |
| swap | `gl/swap.wgsl` | gre | MIT |
| Swirl | `gl/swirl.wgsl` | Sergey Kosarevsky | MIT |
| TilesWave | `gl/tileswave.wgsl` | numb3r23 | MIT |
| TopBottom | `gl/topbottom.wgsl` | zhmy | MIT |
| TVStatic | `gl/tvstatic.wgsl` | Brandon Anzaldi | MIT |
| undulatingBurnOut | `gl/undulatingburnout.wgsl` | pthrasher | MIT |
| VerticalClose | `gl/verticalclose.wgsl` | martiniti | MIT |
| VerticalOpen | `gl/verticalopen.wgsl` | martiniti | MIT |
| WaterDrop | `gl/waterdrop.wgsl` | Paweł Płóciennik | MIT |
| wind | `gl/wind.wgsl` | gre | MIT |
| windowblinds | `gl/windowblinds.wgsl` | Fabien Benetou | MIT |
| windowslice | `gl/windowslice.wgsl` | gre | MIT |
| wipeDown | `gl/wipedown.wgsl` | Jake Nelson | MIT |
| wipeLeft | `gl/wipeleft.wgsl` | Jake Nelson | MIT |
| wipeRight | `gl/wiperight.wgsl` | Jake Nelson | MIT |
| wipeUp | `gl/wipeup.wgsl` | Jake Nelson | MIT |
| x_axis_translation | `gl/x_axis_translation.wgsl` | lizhongjian | MIT |
| ZoomInCircles | `gl/zoomincircles.wgsl` | dycm8009 | MIT |
| zoomInOut | `gl/zoominout.wgsl` | OllyOllyOlly | MIT |
| ZoomLeftWipe | `gl/zoomleftwipe.wgsl` | Handk | MIT |
| ZoomRigthWipe | `gl/zoomrigthwipe.wgsl` | Handk | MIT |

## MIT licence (gl-transitions)

```text
MIT License

Copyright (c) 2017-present gl-transitions contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

Note: Individual transitions in the transitions/ directory may have their own
license specified in their file header comments. When no license is specified,
the transition is covered by this MIT license.
```

## BSD licences

`InvertedPageCurl` (BSD 3-Clause, Copyright (c) 2010 Hewlett-Packard
Development Company, L.P.) and `StereoViewer` (BSD 2-Clause, Copyright (c)
2016, Theodore K Schundler) carry their full licence text in the header of
`gl/invertedpagecurl.wgsl` and `gl/stereoviewer.wgsl`, as their licences
require.
