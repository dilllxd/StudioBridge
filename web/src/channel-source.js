export function channelSourceLabel(channel, studio) {
  if (channel.source_kind === "physical") {
    const link = /^Link\s*([1-4])$/i.exec(channel.name) || /^Link\s*([1-4])$/i.exec(channel.id);
    if (link) {
      const applications = (studio.linked_applications || []).filter((item) => item.channel === `link${link[1]}`);
      return applications.length
        ? `Windows · ${applications.map((item) => item.name).join(" · ")}`
        : `Windows Link ${link[1]} input`;
    }
    if (/mic/i.test(channel.name)) return "Physical input · BEACN Studio microphone";
    return "Physical audio input";
  }
  return channel.applications.length
    ? `Linux · ${channel.applications.join(" · ")}`
    : "Waiting for a Linux application";
}
