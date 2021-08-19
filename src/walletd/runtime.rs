use std::{
    collections::{HashMap, HashSet},
    convert::{TryFrom, TryInto},
    str::FromStr,
};

use crate::rpc::{
    request::{self, Commit, Keys, Msg, Params, Reveal, Token},
    Request, ServiceBus,
};
use crate::swapd::swap_id;
use crate::walletd::NodeSecrets;
use crate::LogStyle;
use crate::Senders;
use crate::{Config, CtlServer, Error, Service, ServiceId};
use bitcoin::{
    hashes::hex::FromHex,
    secp256k1,
    util::{
        bip32::{DerivationPath, ExtendedPrivKey},
        psbt::serialize::Deserialize,
    },
    PrivateKey, PublicKey, Transaction,
};
use colored::Colorize;
use farcaster_core::{
    bitcoin::{segwitv0::FundingTx, segwitv0::SegwitV0, Bitcoin},
    blockchain::FeePolitic,
    bundle::{AliceParameters, BobParameters, CoreArbitratingTransactions, FundingTransaction},
    monero::Monero,
    negotiation::PublicOffer,
    protocol_message::{
        BuyProcedureSignature, CommitAliceParameters, CommitBobParameters, CoreArbitratingSetup,
        RefundProcedureSignatures,
    },
    role::{Alice, Bob, SwapRole, TradeRole},
    swap::btcxmr::{BtcXmr, KeyManager as CoreWallet},
    swap::SwapId,
    transaction::Fundable,
};
use internet2::{LocalNode, ToNodeAddr, TypedEnum, LIGHTNING_P2P_DEFAULT_PORT};
// use lnp::{ChannelId as SwapId, TempChannelId as TempSwapId};
use microservices::esb::{self, Handler};
use request::{LaunchSwap, NodeId};

pub fn run(
    config: Config,
    wallet_token: Token,
    node_secrets: NodeSecrets,
    node_id: bitcoin::secp256k1::PublicKey,
) -> Result<(), Error> {
    let runtime = Runtime {
        identity: ServiceId::Wallet,
        wallet_token,
        node_secrets,
        node_id,
        wallets: none!(),
        swaps: none!(),
    };

    Service::run(config, runtime, false)
}

pub struct Runtime {
    identity: ServiceId,
    wallet_token: Token,
    node_secrets: NodeSecrets,
    node_id: bitcoin::secp256k1::PublicKey,
    wallets: HashMap<SwapId, Wallet>,
    swaps: HashMap<SwapId, Option<Request>>,
}

pub enum Wallet {
    Alice(
        Alice<BtcXmr>,
        AliceParameters<BtcXmr>,
        CoreWallet,
        PublicOffer<BtcXmr>,
        Option<CommitBobParameters<BtcXmr>>,
        Option<BobParameters<BtcXmr>>,
    ),
    Bob(
        Bob<BtcXmr>,
        BobParameters<BtcXmr>,
        CoreWallet,
        PublicOffer<BtcXmr>,
        Option<FundingTx>,
        Option<CommitAliceParameters<BtcXmr>>,
        Option<AliceParameters<BtcXmr>>,
        Option<CoreArbitratingTransactions<Bitcoin<SegwitV0>>>,
    ),
}

impl CtlServer for Runtime {}

impl esb::Handler<ServiceBus> for Runtime {
    type Request = Request;
    type Address = ServiceId;
    type Error = Error;

    fn identity(&self) -> ServiceId {
        self.identity.clone()
    }

    fn handle(
        &mut self,
        senders: &mut esb::SenderList<ServiceBus, ServiceId>,
        bus: ServiceBus,
        source: ServiceId,
        request: Request,
    ) -> Result<(), Self::Error> {
        match bus {
            ServiceBus::Msg => self.handle_rpc_msg(senders, source, request),
            ServiceBus::Ctl => self.handle_rpc_ctl(senders, source, request),
            _ => Err(Error::NotSupported(ServiceBus::Bridge, request.get_type())),
        }
    }

    fn handle_err(&mut self, _: esb::Error) -> Result<(), esb::Error> {
        // We do nothing and do not propagate error; it's already being reported
        // with `error!` macro by the controller. If we propagate error here
        // this will make whole daemon panic
        Ok(())
    }
}

impl Runtime {
    fn send_farcasterd(
        &self,
        senders: &mut Senders,
        message: request::Request,
    ) -> Result<(), Error> {
        senders.send_to(
            ServiceBus::Ctl,
            self.identity(),
            ServiceId::Farcasterd,
            message,
        )?;
        Ok(())
    }

    fn handle_rpc_msg(
        &mut self,
        senders: &mut Senders,
        source: ServiceId,
        request: Request,
    ) -> Result<(), Error> {
        match request.clone() {
            Request::Hello => {
                // Ignoring; this is used to set remote identity at ZMQ level
            }
            // 1st protocol message received through peer connection, and last
            // handled by farcasterd, receiving taker commit because we are
            // maker
            Request::Protocol(Msg::TakerCommit(request::TakeCommit {
                commit: remote_commit,
                public_offer_hex,
                swap_id,
            })) => {
                let public_offer: PublicOffer<BtcXmr> = FromStr::from_str(&public_offer_hex)
                    .map_err(|_| {
                        Error::Farcaster(
                            "The offer received on peer connection is not parsable".to_string(),
                        )
                    })?;
                trace!(
                    "Offer {} is known, you created it previously, initiating swap with taker",
                    &public_offer
                );
                let PublicOffer {
                    version,
                    offer,
                    daemon_service,
                } = public_offer.clone();
                let peer = daemon_service
                    .to_node_addr(internet2::LIGHTNING_P2P_DEFAULT_PORT)
                    .ok_or_else(|| internet2::presentation::Error::InvalidEndpoint)?;
                match offer.maker_role {
                    SwapRole::Bob => {
                        let external_address = address();
                        let bob =
                            Bob::<BtcXmr>::new(external_address.into(), FeePolitic::Aggressive);
                        let core_wallet = CoreWallet::new(self.node_secrets.wallet_seed);
                        let local_params = bob.generate_parameters(&core_wallet, &public_offer)?;
                        if self.wallets.get(&swap_id).is_none() {
                            let funding = create_funding(&self.node_secrets)?;
                            let funding_addr = funding.get_address().expect("funding get_address");
                            info!(
                                "Bob, please send Btc to address: {}",
                                &funding_addr.bright_yellow_bold()
                            );
                            info!("Creating {}", "Wallet::Bob".bright_yellow());
                            if let request::Commit::Alice(remote_commit) = remote_commit.clone() {
                                if self.wallets.get(&swap_id).is_none() {
                                    self.wallets.insert(
                                        swap_id,
                                        Wallet::Bob(
                                            bob,
                                            local_params.clone(),
                                            core_wallet,
                                            public_offer.clone(),
                                            Some(funding),
                                            Some(remote_commit),
                                            None,
                                            None,
                                        ),
                                    );
                                } else {
                                    error!("Wallet already exists");
                                    return Ok(());
                                }
                            } else {
                                error!("Not Commit::Alice");
                                return Ok(());
                            }
                            let launch_swap = LaunchSwap {
                                peer: peer.into(),
                                local_trade_role: TradeRole::Maker,
                                public_offer,
                                local_params: Params::Bob(local_params.clone()),
                                swap_id,
                                remote_commit: Some(remote_commit),
                                funding_address: Some(funding_addr),
                            };
                            self.swaps.insert(swap_id, None);
                            self.send_ctl(
                                senders,
                                ServiceId::Farcasterd,
                                Request::LaunchSwap(launch_swap),
                            )
                        } else {
                            Err(Error::Farcaster("Wallet already existed".to_string()))
                        }
                    }
                    SwapRole::Alice => {
                        let external_address = address();
                        let alice: Alice<BtcXmr> =
                            Alice::new(external_address.into(), FeePolitic::Aggressive);
                        let wallet_seed = self.node_secrets.wallet_seed;
                        let core_wallet = CoreWallet::new(wallet_seed);
                        let params = alice.generate_parameters(&core_wallet, &public_offer)?;
                        if self.wallets.get(&swap_id).is_none() {
                            info!("Creating {}", "Wallet::Alice".bright_yellow());
                            if let request::Commit::Bob(bob_commit) = remote_commit.clone() {
                                if self.wallets.get(&swap_id).is_none() {
                                    self.wallets.insert(
                                        swap_id,
                                        Wallet::Alice(
                                            alice,
                                            params.clone(),
                                            core_wallet,
                                            public_offer.clone(),
                                            Some(bob_commit),
                                            None,
                                        ),
                                    );
                                }
                                let launch_swap = LaunchSwap {
                                    peer: peer.into(),
                                    local_trade_role: TradeRole::Maker,
                                    public_offer,
                                    local_params: Params::Alice(params),
                                    swap_id,
                                    remote_commit: Some(remote_commit),
                                    funding_address: None,
                                };
                                self.send_ctl(
                                    senders,
                                    ServiceId::Farcasterd,
                                    Request::LaunchSwap(launch_swap),
                                )
                            } else {
                                error!("Not Commit::Bob");
                                return Ok(());
                            }
                        } else {
                            error!("Wallet already existed");
                            return Ok(());
                        }
                    }
                }?
            }
            Request::Protocol(Msg::MakerCommit(commit)) => {
                if swap_id(source)? == Msg::MakerCommit(commit.clone()).swap_id() {
                    match commit {
                        Commit::Bob(CommitBobParameters { swap_id, .. }) => {
                            match self.wallets.get_mut(&swap_id) {
                                Some(Wallet::Alice(
                                    _alice,
                                    _alice_params,
                                    core_wallet,
                                    _public_offer,
                                    bob_commit,
                                    bob_params, // None
                                )) => {
                                    if let Some(_) = bob_commit {
                                        error!("Bob commit (remote) already set");
                                    } else if let Commit::Bob(commit) = commit {
                                        trace!("Setting bob commit");
                                        *bob_commit = Some(commit);
                                    }
                                }
                                _ => {
                                    error!("Wallet not found or not on correct state");
                                    return Ok(());
                                }
                            }
                        }
                        Commit::Alice(CommitAliceParameters { swap_id, .. }) => {
                            match self.wallets.get_mut(&swap_id) {
                                Some(Wallet::Bob(
                                    bob,
                                    bob_params,
                                    core_wallet,
                                    public_offer,
                                    funding,
                                    alice_commit, // None
                                    alice_params, // None
                                    core_arb_txs, // None
                                )) => {
                                    if let Some(_) = alice_commit {
                                        error!("Alice commit (remote) already set");
                                    } else if let Commit::Alice(commit) = commit {
                                        trace!("Setting alice commit");
                                        *alice_commit = Some(commit);
                                    }
                                }
                                _ => {
                                    error!("Wallet not found or not on correct state");
                                    return Ok(());
                                }
                            }
                        }
                    }
                }
            }
            Request::Protocol(Msg::Reveal(reveal)) => {
                let swap_id = swap_id(source.clone())?;
                match reveal {
                    // receiving from counterparty Bob, thus I'm Alice (Maker or Taker)
                    Reveal::Bob(reveal) => match self.wallets.get_mut(&swap_id) {
                        Some(Wallet::Alice(
                            _alice,
                            _alice_params,
                            core_wallet,
                            _public_offer,
                            Some(bob_commit),
                            bob_params, // None
                        )) => {
                            if let Some(remote_params) = bob_params {
                                error!("bob_params were previously set to: {}", remote_params);
                                return Ok(());
                            } else {
                                trace!("Setting bob params: {}", reveal);
                                bob_commit.verify_with_reveal(&*core_wallet, reveal.clone())?;
                                *bob_params = Some(reveal.into());
                                // nothing to do yet, waiting for Msg
                                // CoreArbitratingSetup to proceed
                                return Ok(());
                            }
                        }

                        _ => {
                            error!("only Some(Wallet::Alice)");
                            return Ok(());
                        }
                    },
                    // getting parameters from counterparty alice routed through
                    // swapd, thus I'm Bob on this swap: Bob can proceed
                    Reveal::Alice(reveal) => {
                        match self.wallets.get_mut(&swap_id) {
                            Some(Wallet::Bob(
                                bob,
                                bob_params,
                                core_wallet,
                                public_offer,
                                Some(funding),
                                Some(commit),
                                alice_params, // None
                                core_arb_txs, // None
                            )) => {
                                // set wallet params
                                if alice_params.is_some() {
                                    Err(Error::Farcaster("Alice params already set".to_string()))?
                                }
                                *alice_params = Some(reveal.into());

                                // set wallet core_arb_txs
                                if core_arb_txs.is_some() {
                                    Err(Error::Farcaster("Core Arb Txs already set".to_string()))?
                                }
                                // FIXME should be set before
                                let core_arbitrating_txs = bob.core_arbitrating_transactions(
                                    &alice_params.clone().expect("alice_params set above"),
                                    bob_params,
                                    funding.clone(),
                                    public_offer,
                                )?;
                                *core_arb_txs = Some(core_arbitrating_txs.clone());
                                let cosign_arbitrating_cancel = bob.cosign_arbitrating_cancel(
                                    core_wallet,
                                    bob_params,
                                    &core_arbitrating_txs,
                                )?;
                                let core_arb_setup = CoreArbitratingSetup::<BtcXmr>::from((
                                    swap_id,
                                    core_arbitrating_txs,
                                    cosign_arbitrating_cancel,
                                ));
                                let core_arb_setup = Msg::CoreArbitratingSetup(core_arb_setup);
                                self.send_ctl(senders, source, Request::Protocol(core_arb_setup))?;
                            }
                            _ => Err(Error::Farcaster("only Some(Wallet::Bob)".to_string()))?,
                        }
                    }
                }
            }
            Request::Protocol(Msg::RefundProcedureSignatures(refund_proc_sigs)) => {
                let swap_id = swap_id(source.clone())?;

                match self.wallets.get_mut(&swap_id) {
                    Some(Wallet::Bob(
                        bob,
                        bob_params,
                        core_wallet,
                        public_offer,
                        Some(_funding_tx),
                        _commit,
                        Some(alice_params),
                        Some(core_arbitrating_txs),
                    )) => {
                        // *refund_sigs = Some(refund_proc_sigs);
                        let signed_adaptor_buy = bob.sign_adaptor_buy(
                            core_wallet,
                            alice_params,
                            bob_params,
                            core_arbitrating_txs,
                            public_offer,
                        )?;
                        let signed_arb_lock = bob.sign_arbitrating_lock(
                            core_wallet,
                            core_wallet,
                            core_arbitrating_txs,
                        )?;

                        // TODO: here subscribe to all transactions with syncerd, and publish lock
                        let buy_proc_sig =
                            BuyProcedureSignature::<BtcXmr>::from((swap_id, signed_adaptor_buy));
                        let buy_proc_sig = Msg::BuyProcedureSignature(buy_proc_sig);
                        senders.send_to(
                            ServiceBus::Ctl,
                            self.identity(),
                            source, // destination swapd
                            Request::Protocol(buy_proc_sig),
                        )?
                    }
                    _ => Err(Error::Farcaster("Unknown wallet and swap_id".to_string()))?,
                }
            }
            Request::Protocol(Msg::CoreArbitratingSetup(core_arb_setup)) => {
                let swap_id = swap_id(source.clone())?;
                let core_arb_txs = core_arb_setup.into();
                match self.wallets.get(&swap_id) {
                    Some(Wallet::Alice(
                        alice,
                        alice_params,
                        core_wallet,
                        public_offer,
                        _bob_commit,
                        Some(bob_parameters),
                    )) => {
                        let signed_adaptor_refund = alice.sign_adaptor_refund(
                            core_wallet,
                            alice_params,
                            bob_parameters,
                            &core_arb_txs,
                            public_offer,
                        )?;
                        let cosigned_arb_cancel = alice.cosign_arbitrating_cancel(
                            core_wallet,
                            alice_params,
                            bob_parameters,
                            &core_arb_txs,
                            public_offer,
                        )?;
                        let refund_proc_signatures = RefundProcedureSignatures::from((
                            swap_id,
                            cosigned_arb_cancel,
                            signed_adaptor_refund,
                        ));
                        let refund_proc_signatures =
                            Msg::RefundProcedureSignatures(refund_proc_signatures);

                        senders.send_to(
                            ServiceBus::Ctl,
                            self.identity(),
                            source,
                            Request::Protocol(refund_proc_signatures),
                        )?
                    }
                    _ => Err(Error::Farcaster("only Some(Wallet::Alice)".to_string()))?,
                }
            }
            Request::Protocol(Msg::BuyProcedureSignature(buy_proc_sig)) => {
                // TODO: verify signature and if valid create & publish lock transaction
                info!("received buyproceduresignature")
            }
            _ => {
                error!("MSG RPC can only be used for forwarding LNPBP messages")
            }
        }
        Ok(())
    }

    fn handle_rpc_ctl(
        &mut self,
        senders: &mut Senders,
        source: ServiceId,
        request: Request,
    ) -> Result<(), Error> {
        match request {
            Request::Hello => match &source {
                ServiceId::Swap(swap_id) => {
                    if let Some(option_req) = self.swaps.get_mut(&swap_id) {
                        trace!("Know swapd, you launched it");
                        if let Some(req) = option_req {
                            let request = req.clone();
                            *option_req = None;
                            self.send_ctl(senders, source, request)?
                        }
                    }
                }
                source => {
                    debug!("Received Hello from {}", source);
                }
            },
            Request::Progress(progress) => {
                // TODO update wallet state?
                info!("{}", progress);
            }

            Request::TakeOffer(request::PubOffer {
                public_offer,
                peer_secret_key: None,
            }) => {
                let PublicOffer {
                    version,
                    offer,
                    daemon_service,
                } = public_offer.clone();
                let peer = daemon_service
                    .to_node_addr(LIGHTNING_P2P_DEFAULT_PORT)
                    .ok_or_else(|| internet2::presentation::Error::InvalidEndpoint)?;

                let swap_id: SwapId = SwapId::random().into();
                self.swaps.insert(swap_id, None);
                // since we're takers, we are on the other side of the trade
                let taker_role = offer.maker_role.other();
                let core_wallet = CoreWallet::new(self.node_secrets.wallet_seed);
                match taker_role {
                    SwapRole::Bob => {
                        let address = address();
                        let bob: Bob<BtcXmr> = Bob::new(address.into(), FeePolitic::Aggressive);
                        let local_params = bob.generate_parameters(&core_wallet, &public_offer)?;
                        let funding = create_funding(&self.node_secrets)?;
                        let funding_addr = funding.get_address().expect("funding get_address");
                        info!(
                            "Send money to address: {}",
                            funding_addr.bright_yellow_bold()
                        );
                        info!("Creating {}", "Wallet::Bob".bright_yellow());
                        if self.wallets.get(&swap_id).is_none() {
                            self.wallets.insert(
                                swap_id,
                                Wallet::Bob(
                                    bob,
                                    local_params.clone(),
                                    core_wallet,
                                    public_offer.clone(),
                                    Some(funding),
                                    None,
                                    None,
                                    None,
                                ),
                            );
                        } else {
                            Err(Error::Farcaster(s!("Wallet already exists")))?
                        }
                        let launch_swap = LaunchSwap {
                            peer: peer.into(),
                            local_trade_role: TradeRole::Taker,
                            public_offer,
                            local_params: Params::Bob(local_params),
                            swap_id,
                            remote_commit: None,
                            funding_address: Some(funding_addr),
                        };
                        senders.send_to(
                            ServiceBus::Ctl,
                            source,
                            ServiceId::Farcasterd,
                            Request::LaunchSwap(launch_swap),
                        )?;
                    }
                    SwapRole::Alice => {
                        let address = address();
                        let alice: Alice<BtcXmr> =
                            Alice::new(address.into(), FeePolitic::Aggressive);
                        let local_params =
                            alice.generate_parameters(&core_wallet, &public_offer)?;
                        let wallet_seed = self.node_secrets.wallet_seed;
                        let core_wallet = CoreWallet::new(wallet_seed);

                        if self.wallets.get(&swap_id).is_none() {
                            // TODO instead of storing in state, start building
                            // requests and store the state in there directly
                            info!("Creating Alice Taker's Wallet");
                            self.wallets.insert(
                                swap_id,
                                Wallet::Alice(
                                    alice,
                                    local_params.clone(),
                                    core_wallet,
                                    public_offer.clone(),
                                    None,
                                    None,
                                ),
                            );
                        } else {
                            Err(Error::Farcaster(s!("Wallet already exists")))?
                        }
                        let launch_swap = LaunchSwap {
                            peer: peer.into(),
                            local_trade_role: TradeRole::Taker,
                            public_offer,
                            local_params: Params::Alice(local_params),
                            swap_id,
                            remote_commit: None,
                            funding_address: None,
                        };
                        senders.send_to(
                            ServiceBus::Ctl,
                            source,
                            ServiceId::Farcasterd,
                            Request::LaunchSwap(launch_swap),
                        )?;
                    }
                };
            }
            Request::GetKeys(request::GetKeys(wallet_token, request_id)) => {
                // eprintln!("inside PeerSecret handler");
                if wallet_token != self.wallet_token {
                    Err(Error::InvalidToken)?
                }
                info!("sent Secret request to farcasterd");
                self.send_farcasterd(
                    senders,
                    Request::Keys(Keys(
                        self.node_secrets.peerd_secret_key,
                        self.node_secrets.node_id(),
                        request_id,
                    )),
                )?
            }

            _ => {
                error!(
                    "Request {:?} is not supported by the CTL interface",
                    request
                );
            }
        }
        Ok(())
    }
}

fn address() -> bitcoin::Address {
    bitcoin::Address::from_str("tb1qdk49um4fyc7306lp9mhhlkacxz9cmhnr6k8e37")
        .expect("Parsable address")
}

pub fn create_funding(node_secrets: &NodeSecrets) -> Result<FundingTx, Error> {
    let secp = secp256k1::Secp256k1::new();
    let seed = node_secrets.wallet_seed;
    // let master_key =
    //     ExtendedPrivKey::new_master(bitcoin::Network::Bitcoin,
    // &seed).expect("extendedprivkey"); let sk = master_key.derive_priv(&secp,
    // &DerivationPath::from_str("m/0/1'/99").unwrap());
    let sk =
        PrivateKey::from_slice(&seed, bitcoin::Network::Testnet).expect("PrivateKey::from_slice");
    let pk = PublicKey::from_private_key(&secp, &sk);
    let funding = FundingTx::initialize(pk.key, farcaster_core::blockchain::Network::Testnet)
        .map_err(|_| Error::Farcaster("Impossible to initialize funding tx".to_string()))?;
    Ok(funding)
}

pub fn funding_update(funding: &mut FundingTx) {
    // FIXME: this should be received from syncer
    let funding_tx_hex = Vec::from_hex("02000000000101ac0ac2cf357fc5dcb629e9ccdc96ce97cac14cd8c5b97fb6dc0c965d0d698f460100000000fdffffff01d2410f00000000001600142194a75350083e3218d0817ec95be4e043d702bf024730440220160c02a1e23b4eb2e0030d491dbab10c9f73bd63f28c9031e9b0e5405c34f127022030c3fd6fe7da4fb51330c97142d8e9ad8d3da9c731b60099e7a11d71577f57b1012102de38374957449db58aa087abf37fdd8d5722fa00b508afda30513dac2616eb18f2831f00").unwrap();
    let funding_tx = Transaction::deserialize(&funding_tx_hex).unwrap();
    let funding_bundle = FundingTransaction::<Bitcoin<SegwitV0>> {
        funding: funding_tx,
    };
    funding
        .update(funding_bundle.clone().funding.clone())
        .map_err(|_| Error::Farcaster(s!("Could not update funding")))
        .unwrap();
}
