import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import dayjs from 'dayjs';
import { PageAnim, SettingsMenuCard, Switch } from '../../../ui';
import { useMainState } from '../../../contexts';
import {
  MixnetData,
  Tunnel,
  WgNode,
  WireguardData,
  isMixnetData,
  isWireguardData,
} from '../../../types';
import NetworkEnvSelect from './NetworkEnvSelect';

function Dev() {
  const [credentialsMode, setCredentialsMode] = useState(false);

  const { daemonStatus, networkEnv, tunnel, state } = useMainState();

  useEffect(() => {
    const getCredentialsMode = async () => {
      const enabled = await invoke<boolean>('get_credentials_mode');
      console.log('credentials mode:', enabled);
      setCredentialsMode(enabled);
    };
    getCredentialsMode();
  }, []);

  const credentialsModeChanged = (enabled: boolean) => {
    invoke('set_credentials_mode', { enabled }).then(() => {
      setCredentialsMode(enabled);
    });
  };

  const DataBlock = ({
    children,
    title,
  }: {
    children: React.ReactNode;
    title: string;
  }) => (
    <div>
      <h3 className="text-lg mb-2">{title}</h3>
      <BlockMono>{children}</BlockMono>
    </div>
  );

  const BlockMono = ({ children }: { children: React.ReactNode }) => (
    <div className="bg-black/20 rounded-md flex font-mono text-base overflow-x-scroll">
      <div className="p-3 flex flex-col gap-2 select-text cursor-text">
        {children}
      </div>
    </div>
  );

  const tunnelOverview = (tunnel: Tunnel) => (
    <DataBlock title="tunnel">
      <div>
        entry gw:
        <div>{tunnel.entryGwId}</div>
      </div>
      <div>
        exit gw:
        <div>{tunnel.exitGwId}</div>
      </div>
      {tunnel.connectedAt &&
        `connectedAt: ${dayjs.unix(tunnel.connectedAt).format()}`}
    </DataBlock>
  );

  const mixnetData = (data: MixnetData) => (
    <DataBlock title="mixnet data">
      {data.nymAddress && (
        <>
          nym address:
          <div>{data.nymAddress?.nymAddress}</div>
        </>
      )}
      {data.exitIpr && (
        <>
          exit ipr:
          <div>{data.exitIpr?.nymAddress}</div>
        </>
      )}
      <div>{`ipv4: ${data.ipv4}`}</div>
      <div>{`ipv6: ${data.ipv6}`}</div>
      <div>{`entry ip: ${data.entryIp}`}</div>
      <div>{`exit ip: ${data.exitIp}`}</div>
    </DataBlock>
  );

  const wgNode = (node: WgNode) => (
    <div>
      <div>{`endpoint: ${node.endpoint}`}</div>
      <div>{`private ipv4: ${node.privateIpv4}`}</div>
      <div>{`private ipv6: ${node.privateIpv6}`}</div>
      {'pub key:'}
      <div>{node.publicKey}</div>
    </div>
  );

  const wgData = (data: WireguardData) => (
    <DataBlock title="wg data">
      <div>
        ENTRY:
        {wgNode(data.entry)}
      </div>
      <div className="mt-1">
        EXIT:
        {wgNode(data.exit)}
      </div>
    </DataBlock>
  );

  return (
    <PageAnim className="h-full flex flex-col py-6 gap-6 select-none cursor-default">
      <SettingsMenuCard
        title={'CREDENTIALS_MODE'}
        onClick={() => credentialsModeChanged(!credentialsMode)}
        trailingComponent={
          <Switch checked={credentialsMode} onChange={credentialsModeChanged} />
        }
      />
      {daemonStatus !== 'down' && networkEnv && (
        <NetworkEnvSelect current={networkEnv} />
      )}
      <DataBlock title="state">{state}</DataBlock>
      {tunnel && tunnelOverview(tunnel)}
      {tunnel && isMixnetData(tunnel.data) && mixnetData(tunnel.data)}
      {tunnel && isWireguardData(tunnel.data) && wgData(tunnel.data)}
    </PageAnim>
  );
}

export default Dev;
