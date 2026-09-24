import { Cable, Globe, HelpCircle, Usb, Wifi, type LucideIcon } from 'lucide-react';
import { Status, type NetworkLink, type NetworkLinkPairInfo } from '../../core/types';
import { useAppStore } from '../../stores/app-store';

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

type NetworkLinkBadgeViewProps = {
  linkPair: NetworkLinkPairInfo | null;
  withLeadingSeparator?: boolean;
};

export function NetworkLinkBadgeView({
  linkPair,
  withLeadingSeparator = false,
}: NetworkLinkBadgeViewProps) {
  if (!linkPair) return null;
  const effective = getLinkMeta(linkPair.effective);
  const phone = getLinkMeta(linkPair.phone);
  const pc = getLinkMeta(linkPair.pc);
  const EffectiveIcon = effective.icon;
  const title =
    linkPair.phone === linkPair.pc
      ? `Link: ${effective.label}`
      : `Phone ${phone.label}, PC ${pc.label} — buffer tuned for ${effective.label}`;

  return (
    <div
      id="network-link-badge"
      className="inline-flex min-w-0 items-center gap-1.5 text-[11px] font-medium"
      title={title}
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

type NetworkLinkBadgeProps = {
  withLeadingSeparator?: boolean;
};

export function NetworkLinkBadge({ withLeadingSeparator = false }: NetworkLinkBadgeProps = {}) {
  const linkPair = useAppStore((state) => state.networkLinkPair);
  const status = useAppStore((state) => state.status);
  const visible = [Status.Connected, Status.Playing, Status.Paused].includes(status);
  return (
    <NetworkLinkBadgeView
      linkPair={visible ? linkPair : null}
      withLeadingSeparator={withLeadingSeparator}
    />
  );
}
