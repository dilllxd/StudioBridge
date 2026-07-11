# Mixer visual brief

The generated reference in `mixer-reference.png` is the visual source of truth.
`mixer-readonly-reference.png` is the safety-state variant: it adds a restrained
amber `READ-ONLY` label and visibly disables gain, phantom-power, and Link
assignment writes without dimming PipeWeaver mixer controls.

- Full-viewport operator console: 68 px top bar, 178 px navigation rail,
  flexible mixer deck, and 350 px Studio inspector.
- Graphite-black surfaces separated by hairline warm-gray borders. The interface
  should feel like powder-coated broadcast hardware, not floating web cards.
- Condensed uppercase channel headings with a readable humanist sans for values
  and controls.
- Mineral teal always represents the Personal mix. Signal orange always
  represents the Audience mix. Red is reserved for destructive/muted state.
- Each channel is a narrow vertical strip with paired faders, paired meters,
  source names, and one large mute control at the bottom.
- The right inspector remains fixed and exposes the controls most likely to be
  needed mid-stream: gain, phantom power, headphone level, and Link assignment.
- Controls use square corners, tactile borders, and restrained inset depth.
- At narrower viewport widths, the mixer deck scrolls horizontally while the
  main navigation and Studio inspector remain available.
