import { useAppStore } from '../../stores/app-store';
import { Status } from '../../core/types';
import type { NetworkLink } from '../../core/types';
import { Usb, Wifi, Globe, Cable, HelpCircle } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';

type LinkMeta = {
  icon: LucideIcon;
  label: string;
  color: string;
};

function getLinkMeta(link: NetworkLink): LinkMeta {
  switch (link) {
    case 'adb':
      return { icon: Cable, label: 'ADB', color: 'text-accent-green' };
    case 'usbTether':
      return { icon: Usb, label: 'USB', color: 'text-accent-green' };
    case 'wifi5Ghz':
      return { icon: Wifi, label: '5 GHz', color: 'text-accent-aqua' };
    case 'wifi2_4Ghz':
      return { icon: Wifi, label: '2.4 GHz', color: 'text-accent-yellow' };
    case 'ethernet':
      return { icon: Globe, label: 'Ethernet', color: 'text-accent-aqua' };
    case 'wifiUnknown':
      return { icon: Wifi, label: 'WiFi', color: 'text-muted-foreground' };
    default:
      return { icon: HelpCircle, label: 'Unknown', color: 'text-muted-foreground/60' };
  }
}

type NetworkLinkBadgeProps = {
  /**
   * Render a leading `|` rule. Opt-in because the badge is self-hiding — the
   * caller cannot know whether it will render, so it has to own the separator or
   * a stray divider is left behind when there is no link to show.
   */
  withLeadingSeparator?: boolean;
};

export function NetworkLinkBadge({ withLeadingSeparator = false }: NetworkLinkBadgeProps = {}) {
  const linkPair = useAppStore((s) => s.networkLinkPair);
  const status = useAppStore((s) => s.status);

  const visible =
    linkPair &&
    (status === Status.Connected || status === Status.Playing || status === Status.Paused);

  if (!visible) return null;

  const effective = getLinkMeta(linkPair.effective);
  const EffectiveIcon = effective.icon;

  // Named per side rather than as "effective vs the other one", so the tooltip
  // stays correct when `effective` is neither side (rule 4, above).
  const isSymmetric = linkPair.phone === linkPair.pc;
  const phone = getLinkMeta(linkPair.phone);
  const pc = getLinkMeta(linkPair.pc);

  return (
    <div
      id="network-link-badge"
      className="inline-flex min-w-0 items-center gap-1.5 text-[11px] font-medium"
      title={
        isSymmetric
          ? `Link: ${effective.label}`
          : `Phone ${phone.label}, PC ${pc.label} — buffer tuned for ${effective.label}`
      }
    >
      {withLeadingSeparator && (
        <span aria-hidden="true" className="text-muted-foreground/40">
          |
        </span>
      )}
      <EffectiveIcon size={12} className={`shrink-0 ${effective.color}`} aria-hidden="true" />
      <span className={`truncate ${effective.color}`}>{effective.label}</span>
    </div>
  );
}
