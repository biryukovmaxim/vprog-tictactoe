/* tslint:disable */
/* eslint-disable */
/**
 * Parses a network name (`simnet`, `testnet-10`) into shared consensus parameters.
 */
export function network_params(network: string): JsParams;
/**
 * Derives the lock hash, user resource id, and x-only public key for a private key hex.
 */
export function my_ids(privkey_hex: string): MyIds;
/**
 * Builds a signed `CreateGame` carrier; a positive `deposit_amount` prepends a `Deposit` action
 * citing the covenant deposit output this transaction carries at index 0.
 */
export function create_game_tx(privkey_hex: string, net: JsParams, utxo: UtxoCandidate, change_address: string, lane_subnet_hex: string, config_id_hex: string, my_games_started: bigint, stake: bigint, rounds: number, mark_u8: number, deposit_amount: bigint, covenant_id_hex: string): Uint8Array;
/**
 * Builds a signed `JoinGame` carrier; a positive `deposit_amount` prepends a `Deposit` action
 * citing the covenant deposit output this transaction carries at index 0.
 */
export function join_game_tx(privkey_hex: string, net: JsParams, utxo: UtxoCandidate, change_address: string, lane_subnet_hex: string, config_id_hex: string, game_id_hex: string, deposit_amount: bigint, covenant_id_hex: string): Uint8Array;
/**
 * Builds a signed single-`Turn` carrier playing `cell` (0..=8) in the caller's game.
 */
export function turn_tx(privkey_hex: string, net: JsParams, utxo: UtxoCandidate, change_address: string, lane_subnet_hex: string, game_id_hex: string, my_user_id_hex: string, opponent_user_id_hex: string, cell: number): Uint8Array;
/**
 * Builds a signed `Transfer` carrier from the caller's user to `dest_user_id_hex`; a new
 * destination additionally requires its x-only public key to bind its lock.
 */
export function transfer_tx(privkey_hex: string, net: JsParams, utxo: UtxoCandidate, change_address: string, lane_subnet_hex: string, dest_user_id_hex: string, dest_exists: boolean, amount: bigint, dest_pubkey_hex?: string | null): Uint8Array;
/**
 * Builds a signed `Withdraw` carrier exiting `amount` to the caller's own public key.
 */
export function withdraw_tx(privkey_hex: string, net: JsParams, utxo: UtxoCandidate, change_address: string, lane_subnet_hex: string, config_id_hex: string, amount: bigint): Uint8Array;
/**
 * Builds a full-leaf permission-tree claim spending the settled exit leaf; `delegate_utxos` are
 * covenant deposit UTXOs funding the payout, and `fee` burns delegate value for relay priority.
 */
export function claim_tx(covenant_id_hex: string, permission_txid_hex: string, permission_index: number, permission_rent: bigint, old_root_hex: string, old_unclaimed: bigint, depth: number, leaf_index: number, leaf_spk_hex: string, leaf_amount: bigint, new_root_hex: string, new_unclaimed: bigint, siblings_hex: string[], delegate_utxos: UtxoCandidate[], fee: bigint): Uint8Array;
/**
 * r" Deferred promise - an object that has `resolve()` and `reject()`
 * r" functions that can be called outside of the promise body.
 * r" WARNING: This function uses `eval` and can not be used in environments
 * r" where dynamically-created code can not be executed such as web browser
 * r" extensions.
 * r" @category General
 */
export function defer(): Promise<any>;
/**
 * Initialize Rust panic handler in console mode.
 *
 * This will output additional debug information during a panic to the console.
 * This function should be called right after loading WASM libraries.
 * @category General
 */
export function initConsolePanicHook(): void;
/**
 * Initialize Rust panic handler in browser mode.
 *
 * This will output additional debug information during a panic in the browser
 * by creating a full-screen `DIV`. This is useful on mobile devices or where
 * the user otherwise has no access to console/developer tools. Use
 * {@link presentPanicHookLogs} to activate the panic logs in the
 * browser environment.
 * @see {@link presentPanicHookLogs}
 * @category General
 */
export function initBrowserPanicHook(): void;
/**
 * Present panic logs to the user in the browser.
 *
 * This function should be called after a panic has occurred and the
 * browser-based panic hook has been activated. It will present the
 * collected panic logs in a full-screen `DIV` in the browser.
 * @see {@link initBrowserPanicHook}
 * @category General
 */
export function presentPanicHookLogs(): void;
/**
 * Configuration for the WASM32 bindings runtime interface.
 * @see {@link IWASM32BindingsConfig}
 * @category General
 */
export function initWASM32Bindings(config: IWASM32BindingsConfig): void;
/**
 * Set the logger log level using a string representation.
 * Available variants are: 'off', 'error', 'warn', 'info', 'debug', 'trace'
 * @category General
 */
export function setLogLevel(level: "off" | "error" | "warn" | "info" | "debug" | "trace"): void;
/**
 *
 *  Kaspa `Address` version (`PubKey`, `PubKey ECDSA`, `ScriptHash`)
 *
 * @category Address
 */
export enum AddressVersion {
  /**
   * PubKey addresses always have the version byte set to 0
   */
  PubKey = 0,
  /**
   * PubKey ECDSA addresses always have the version byte set to 1
   */
  PubKeyECDSA = 1,
  /**
   * ScriptHash addresses always have the version byte set to 8
   */
  ScriptHash = 8,
}
/**
 * `ConnectionStrategy` specifies how the WebSocket `async fn connect()`
 * function should behave during the first-time connectivity phase.
 * @category WebSocket
 */
export enum ConnectStrategy {
  /**
   * Continuously attempt to connect to the server. This behavior will
   * block `connect()` function until the connection is established.
   */
  Retry = 0,
  /**
   * Causes `connect()` to return immediately if the first-time connection
   * has failed.
   */
  Fallback = 1,
}
/**
 * wRPC protocol encoding: `Borsh` or `JSON`
 * @category Transport
 */
export enum Encoding {
  Borsh = 0,
  SerdeJson = 1,
}
/**
 * @category Consensus
 */
export enum NetworkType {
  Mainnet = 0,
  Testnet = 1,
  Devnet = 2,
  Simnet = 3,
}

/**
 * Interface defines the structure of a UTXO entry.
 * 
 * @category Consensus
 */
export interface IUtxoEntry {
    /** @readonly */
    address?: Address;
    /** @readonly */
    outpoint: ITransactionOutpoint;
    /** @readonly */
    amount : bigint;
    /** @readonly */
    scriptPublicKey : IScriptPublicKey;
    /** @readonly */
    blockDaaScore: bigint;
    /** @readonly */
    isCoinbase: boolean;
}




/**
 * Interface defining the structure of a transaction.
 *
 * @category Consensus
 */
export interface ITransaction {
    version: number;
    inputs: ITransactionInput[];
    outputs: ITransactionOutput[];
    lockTime: bigint;
    subnetworkId: HexString;
    gas: bigint;
    payload: HexString;

    /**
     * @deprecated since version 1.3.0, use `storageMass`
    */
    mass?: bigint;

    /** The mass of the transaction (the mass is undefined or zero unless explicitly set or obtained from the node) */
    storageMass?: bigint;

    /** Optional verbose data provided by RPC */
    verboseData?: ITransactionVerboseData;
}

/**
 * Optional transaction verbose data.
 *
 * @category Node RPC
 */
export interface ITransactionVerboseData {
    transactionId : HexString;
    hash : HexString;
    computeMass : bigint;
    blockHash : HexString;
    blockTime : bigint;
}




/**
 * Interface defines the structure of a serializable UTXO entry.
 * 
 * @see {@link ISerializableTransactionInput}, {@link ISerializableTransaction}
 * @category Wallet SDK
 */
export interface ISerializableUtxoEntry {
    address?: Address;
    amount: bigint;
    scriptPublicKey: ScriptPublicKey;
    blockDaaScore: bigint;
    isCoinbase: boolean;
}

/**
 * Interface defines the structure of a serializable transaction input.
 * 
 * @see {@link ISerializableTransaction}
 * @category Wallet SDK
 */
export interface ISerializableTransactionInput {
    transactionId : HexString;
    index: number;
    sequence: bigint;
    sigOpCount: number;
    computeBudget?: number;
    signatureScript?: HexString;
    utxo: ISerializableUtxoEntry;
}

/**
 * Interface defines the structure of a serializable transaction output.
 * 
 * @see {@link ISerializableTransaction}
 * @category Wallet SDK
 */
export interface ISerializableTransactionOutput {
    value: bigint;
    scriptPublicKey: IScriptPublicKey;
}

/**
 * Interface defines the structure of a serializable transaction.
 * 
 * Serializable transactions can be produced using 
 * {@link Transaction.serializeToJSON},
 * {@link Transaction.serializeToSafeJSON} and 
 * {@link Transaction.serializeToObject} 
 * functions for processing (signing) in external systems.
 * 
 * Once the transaction is signed, it can be deserialized
 * into {@link Transaction} using {@link Transaction.deserializeFromJSON}
 * and {@link Transaction.deserializeFromSafeJSON} functions. 
 * 
 * @see {@link Transaction},
 * {@link ISerializableTransactionInput},
 * {@link ISerializableTransactionOutput},
 * {@link ISerializableUtxoEntry}
 * 
 * @category Wallet SDK
 */
export interface ISerializableTransaction {
    id? : HexString;
    version: number;
    inputs: ISerializableTransactionInput[];
    outputs: ISerializableTransactionOutput[];
    lockTime: bigint;
    subnetworkId: HexString;
    gas: bigint;
    payload: HexString;
}




/**
 * Interface defines the structure of a transaction outpoint (used by transaction input).
 * 
 * @category Consensus
 */
export interface ITransactionOutpoint {
    transactionId: HexString;
    index: number;
}



/**
 * Interface defines the structure of a transaction input.
 * 
 * @category Consensus
 */
export interface ITransactionInput {
    previousOutpoint: ITransactionOutpoint;
    signatureScript?: HexString;
    sequence: bigint;
    sigOpCount: number;
    computeBudget?: number;
    utxo?: UtxoEntryReference;

    /** Optional verbose data provided by RPC */
    verboseData?: ITransactionInputVerboseData;
}

/**
 * Option transaction input verbose data.
 * 
 * @category Node RPC
 */
export interface ITransactionInputVerboseData { }




/**
 * Interface defining the structure of a transaction output.
 * 
 * @category Consensus
 */
export interface ITransactionOutput {
    value: bigint;
    scriptPublicKey: IScriptPublicKey | HexString;

    /** Optional verbose data provided by RPC */
    verboseData?: ITransactionOutputVerboseData;
}

/**
 * TransactionOutput verbose data.
 * 
 * @category Node RPC
 */
export interface ITransactionOutputVerboseData {
    scriptPublicKeyType : string;
    scriptPublicKeyAddress : string;
}



/**
 * A covenant binding binds a transaction output to the covenant and input authorizing its creation.
 *
 * @category Consensus
 */
export interface ICovenantBinding {
    authorizingInput: number;
    covenantId: HexString;
}



/**
 * A genesis covenant group for bulk covenant binding population.
 *
 * @category Consensus
 */
export interface IGenesisCovenantGroup {
    authorizingInput: number;
    outputs: number[];
}



/**
 * Color range configuration for Hex View.
 * 
 * @category General
 */ 
export interface IHexViewColor {
    start: number;
    end: number;
    color?: string;
    background?: string;
}

/**
 * Configuration interface for Hex View.
 * 
 * @category General
 */ 
export interface IHexViewConfig {
    offset? : number;
    replacementCharacter? : string;
    width? : number;
    colors? : IHexViewColor[];
}



/**
 * A string containing a hexadecimal representation of the data (typically representing for IDs or Hashes).
 * 
 * @category General
 */ 
export type HexString = string;



/**
 * Interface defines the structure of a Script Public Key.
 * 
 * @category Consensus
 */
export interface IScriptPublicKey {
    version : number;
    script: HexString;
}



    /**
     * Generic network address representation.
     * 
     * @category General
     */
    export interface INetworkAddress {
        /**
         * IPv4 or IPv6 address.
         */
        ip: string;
        /**
         * Optional port number.
         */
        port?: number;
    }



/**
 * Interface for configuring workflow-rs WASM32 bindings.
 * 
 * @category General
 */
export interface IWASM32BindingsConfig {
    /**
     * This option can be used to disable the validation of class names
     * for instances of classes exported by Rust WASM32 when passing
     * these classes to WASM32 functions.
     * 
     * This can be useful to programmatically disable checks when using
     * a bundler that mangles class symbol names.
     */
    validateClassNames : boolean;
}


/**
 *
 * Abortable trigger wraps an `Arc<AtomicBool>`, which can be cloned
 * to signal task terminating using an atomic bool.
 *
 * ```text
 * let abortable = Abortable::default();
 * let result = my_task(abortable).await?;
 * // ... elsewhere
 * abortable.abort();
 * ```
 *
 * @category General
 */
export class Abortable {
  free(): void;
  isAborted(): boolean;
  constructor();
  abort(): void;
  check(): void;
  reset(): void;
}
/**
 * Error emitted by [`Abortable`].
 * @category General
 */
export class Aborted {
  private constructor();
  free(): void;
}
/**
 * Kaspa [`Address`] struct that serializes to and from an address format string: `kaspa:qz0s...t8cv`.
 *
 * @category Address
 */
export class Address {
/**
** Return copy of self without private attributes.
*/
  toJSON(): Object;
/**
* Return stringified version of self.
*/
  toString(): string;
  free(): void;
  constructor(address: string);
  /**
   * Convert an address to a string.
   */
  toString(): string;
  static validate(address: string): boolean;
  readonly prefix: string;
  readonly payload: string;
  readonly version: string;
  set setPrefix(value: string);
}
export class CovenantBinding {
/**
** Return copy of self without private attributes.
*/
  toJSON(): Object;
/**
* Return stringified version of self.
*/
  toString(): string;
  free(): void;
  toJSON(): object;
  constructor(authorizing_input: number, covenant_id: Hash);
  covenantId: Hash;
  authorizingInput: number;
}
/**
 * A genesis covenant group for bulk covenant binding population.
 *
 * All listed outputs are bound to the same covenant id, derived from the
 * authorizing input outpoint and this exact ordered output list.
 * @category Consensus
 */
export class GenesisCovenantGroup {
/**
** Return copy of self without private attributes.
*/
  toJSON(): Object;
/**
* Return stringified version of self.
*/
  toString(): string;
  free(): void;
  toString(): string;
  toJSON(): object;
  constructor(authorizing_input: number, outputs: Array<number>);
  outputs: Array<number>;
  authorizingInput: number;
}
/**
 * @category General
 */
export class Hash {
  free(): void;
  constructor(hex_str: string);
  toString(): string;
}
/**
 * Parsed network parameters, shared across every tx-builder call.
 */
export class JsParams {
  private constructor();
  free(): void;
  /**
   * Address prefix string for this network (e.g. `kaspasim`).
   */
  readonly prefix: string;
}
/**
 * The caller's derived on-rollup identity for one private key.
 */
export class MyIds {
  private constructor();
  free(): void;
  /**
   * Schnorr lock identity hash hex the user resource derives from.
   */
  lock_hash_hex: string;
  /**
   * User resource id hex.
   */
  user_id_hex: string;
  /**
   * X-only public key hex.
   */
  pubkey_hex: string;
}
/**
 *
 * NetworkId is a unique identifier for a kaspa network instance.
 * It is composed of a network type and an optional suffix.
 *
 * @category Consensus
 */
export class NetworkId {
/**
** Return copy of self without private attributes.
*/
  toJSON(): Object;
/**
* Return stringified version of self.
*/
  toString(): string;
  free(): void;
  toString(): string;
  addressPrefix(): string;
  constructor(value: any);
  type: NetworkType;
  get suffix(): number | undefined;
  set suffix(value: number | null | undefined);
  readonly id: string;
}
/**
 *
 * Data structure representing a Node connection endpoint
 * as provided by the {@link Resolver}.
 *
 * @category Node RPC
 */
export class NodeDescriptor {
  private constructor();
/**
** Return copy of self without private attributes.
*/
  toJSON(): Object;
/**
* Return stringified version of self.
*/
  toString(): string;
  free(): void;
  /**
   * The unique identifier of the node.
   */
  uid: string;
  /**
   * The URL of the node WebSocket (wRPC URL).
   */
  url: string;
}
/**
 * Represents a Kaspad ScriptPublicKey
 * @category Consensus
 */
export class ScriptPublicKey {
/**
** Return copy of self without private attributes.
*/
  toJSON(): Object;
/**
* Return stringified version of self.
*/
  toString(): string;
  free(): void;
  constructor(version: number, script: any);
  version: number;
  readonly script: string;
}
export class SigHashType {
  private constructor();
  free(): void;
}
/**
 * Represents a Kaspa transaction.
 * This is an artificial construct that includes additional
 * transaction-related data such as additional data from UTXOs
 * used by transaction inputs.
 * @category Consensus
 */
export class Transaction {
/**
** Return copy of self without private attributes.
*/
  toJSON(): Object;
/**
* Return stringified version of self.
*/
  toString(): string;
  free(): void;
  constructor(js_value: ITransaction | Transaction);
  /**
   * Determines whether or not a transaction is a coinbase transaction. A coinbase
   * transaction is a special transaction created by miners that distributes fees and block subsidy
   * to the previous blocks' miners, and specifies the script_pub_key that will be used to pay the current
   * miner in future blocks.
   */
  is_coinbase(): boolean;
  populateGenesisCovenants(groups: (IGenesisCovenantGroup | GenesisCovenantGroup)[]): void;
  /**
   * Recompute and finalize the tx id based on updated tx fields
   */
  finalize(): Hash;
  /**
   * Returns a list of unique addresses used by transaction inputs.
   * This method can be used to determine addresses used by transaction inputs
   * in order to select private keys needed for transaction signing.
   */
  addresses(network_type: NetworkType | NetworkId | string): Address[];
  /**
   * Serializes the transaction to a JSON string.
   * The schema of the JSON is defined by {@link ISerializableTransaction}.
   */
  serializeToJSON(): string;
  /**
   * Serializes the transaction to a pure JavaScript Object.
   * The schema of the JavaScript object is defined by {@link ISerializableTransaction}.
   * @see {@link ISerializableTransaction}
   */
  serializeToObject(): ISerializableTransaction;
  /**
   * Deserialize the {@link Transaction} Object from a JSON string.
   */
  static deserializeFromJSON(json: string): Transaction;
  /**
   * Serializes the transaction to a "Safe" JSON schema where it converts all `bigint` values to `string` to avoid potential client-side precision loss.
   */
  serializeToSafeJSON(): string;
  /**
   * Deserialize the {@link Transaction} Object from a pure JavaScript Object.
   */
  static deserializeFromObject(js_value: any): Transaction;
  /**
   * Deserialize the {@link Transaction} Object from a "Safe" JSON schema where all `bigint` values are represented as `string`.
   */
  static deserializeFromSafeJSON(json: string): Transaction;
  version: number;
  lockTime: bigint;
  storageMass: bigint;
  get inputs(): TransactionInput[];
  set inputs(value: (ITransactionInput | TransactionInput)[]);
  get outputs(): TransactionOutput[];
  set outputs(value: (ITransactionOutput | TransactionOutput)[]);
  get subnetworkId(): string;
  set subnetworkId(value: any);
  get payload(): string;
  set payload(value: any);
  gas: bigint;
  /**
   * @deprecated Use `storageMass` instead
   */
  mass: bigint;
  /**
   * Returns the transaction ID
   */
  readonly id: string;
}
/**
 * Represents a Kaspa transaction input
 * @category Consensus
 */
export class TransactionInput {
/**
** Return copy of self without private attributes.
*/
  toJSON(): Object;
/**
* Return stringified version of self.
*/
  toString(): string;
  free(): void;
  constructor(value: ITransactionInput | TransactionInput);
  sequence: bigint;
  sigOpCount: number;
  computeBudget: number;
  get previousOutpoint(): TransactionOutpoint;
  set previousOutpoint(value: any);
  get signatureScript(): string | undefined;
  set signatureScript(value: any);
  readonly utxo: UtxoEntryReference | undefined;
}
/**
 * Represents a Kaspa transaction outpoint.
 * NOTE: This struct is immutable - to create a custom outpoint
 * use the `TransactionOutpoint::new` constructor. (in JavaScript
 * use `new TransactionOutpoint(transactionId, index)`).
 * @category Consensus
 */
export class TransactionOutpoint {
  private constructor();
/**
** Return copy of self without private attributes.
*/
  toJSON(): Object;
/**
* Return stringified version of self.
*/
  toString(): string;
  free(): void;
}
/**
 * Represents a Kaspad transaction output
 * @category Consensus
 */
export class TransactionOutput {
/**
** Return copy of self without private attributes.
*/
  toJSON(): Object;
/**
* Return stringified version of self.
*/
  toString(): string;
  free(): void;
  /**
   * TransactionOutput constructor
   */
  constructor(value: bigint, script_public_key: ScriptPublicKey, covenant?: CovenantBinding | null);
  get covenant(): CovenantBinding | undefined;
  set covenant(value: CovenantBinding);
  scriptPublicKey: ScriptPublicKey;
  value: bigint;
}
/**
 * Holds details about an individual transaction output in a utxo
 * set such as whether or not it was contained in a coinbase tx, the daa
 * score of the block that accepts the tx, its public key script, and how
 * much it pays.
 * @category Consensus
 */
export class TransactionUtxoEntry {
  private constructor();
/**
** Return copy of self without private attributes.
*/
  toJSON(): Object;
/**
* Return stringified version of self.
*/
  toString(): string;
  free(): void;
  amount: bigint;
  scriptPublicKey: ScriptPublicKey;
  blockDaaScore: bigint;
  isCoinbase: boolean;
  get covenantId(): Hash | undefined;
  set covenantId(value: Hash | null | undefined);
}
/**
 * One spendable L1 UTXO backing a carrier input, as fetched from the wallet.
 */
export class UtxoCandidate {
  private constructor();
  free(): void;
  /**
   * Transaction id of the funding output, in display hex form.
   */
  txid_hex: string;
  /**
   * Output index within the funding transaction.
   */
  index: number;
  /**
   * Output value in sompi.
   */
  amount: bigint;
  /**
   * Output script public key bytes in hex.
   */
  spk_hex: string;
  /**
   * Output script public key version.
   */
  spk_version: number;
}
/**
 * A simple collection of UTXO entries. This struct is used to
 * retain a set of UTXO entries in the WASM memory for faster
 * processing. This struct keeps a list of entries represented
 * by `UtxoEntryReference` struct. This data structure is used
 * internally by the framework, but is exposed for convenience.
 * Please consider using `UtxoContext` instead.
 * @category Wallet SDK
 */
export class UtxoEntries {
/**
** Return copy of self without private attributes.
*/
  toJSON(): Object;
/**
* Return stringified version of self.
*/
  toString(): string;
  free(): void;
  /**
   * Sort the contained entries by amount. Please note that
   * this function is not intended for use with large UTXO sets
   * as it duplicates the whole contained UTXO set while sorting.
   */
  sort(): void;
  amount(): bigint;
  /**
   * Create a new `UtxoEntries` struct with a set of entries.
   */
  constructor(js_value: any);
  items: any;
}
/**
 * [`UtxoEntry`] struct represents a client-side UTXO entry.
 *
 * @category Wallet SDK
 */
export class UtxoEntry {
  private constructor();
/**
** Return copy of self without private attributes.
*/
  toJSON(): Object;
/**
* Return stringified version of self.
*/
  toString(): string;
  free(): void;
  toString(): string;
  get address(): Address | undefined;
  set address(value: Address | null | undefined);
  outpoint: TransactionOutpoint;
  amount: bigint;
  scriptPublicKey: ScriptPublicKey;
  blockDaaScore: bigint;
  isCoinbase: boolean;
  get covenantId(): Hash | undefined;
  set covenantId(value: Hash | null | undefined);
}
/**
 * [`Arc`] reference to a [`UtxoEntry`] used by the wallet subsystems.
 *
 * @category Wallet SDK
 */
export class UtxoEntryReference {
  private constructor();
/**
** Return copy of self without private attributes.
*/
  toJSON(): Object;
/**
* Return stringified version of self.
*/
  toString(): string;
  free(): void;
  toString(): string;
  readonly isCoinbase: boolean;
  readonly blockDaaScore: bigint;
  readonly scriptPublicKey: ScriptPublicKey;
  readonly entry: UtxoEntry;
  readonly amount: bigint;
  readonly address: Address | undefined;
  readonly outpoint: TransactionOutpoint;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
  readonly memory: WebAssembly.Memory;
  readonly __wbg_get_myids_lock_hash_hex: (a: number) => [number, number];
  readonly __wbg_get_myids_pubkey_hex: (a: number) => [number, number];
  readonly __wbg_get_myids_user_id_hex: (a: number) => [number, number];
  readonly __wbg_get_utxocandidate_amount: (a: number) => bigint;
  readonly __wbg_get_utxocandidate_index: (a: number) => number;
  readonly __wbg_get_utxocandidate_spk_version: (a: number) => number;
  readonly __wbg_jsparams_free: (a: number, b: number) => void;
  readonly __wbg_myids_free: (a: number, b: number) => void;
  readonly __wbg_set_myids_lock_hash_hex: (a: number, b: number, c: number) => void;
  readonly __wbg_set_myids_pubkey_hex: (a: number, b: number, c: number) => void;
  readonly __wbg_set_myids_user_id_hex: (a: number, b: number, c: number) => void;
  readonly __wbg_set_utxocandidate_amount: (a: number, b: bigint) => void;
  readonly __wbg_set_utxocandidate_index: (a: number, b: number) => void;
  readonly __wbg_set_utxocandidate_spk_version: (a: number, b: number) => void;
  readonly __wbg_set_utxocandidate_txid_hex: (a: number, b: number, c: number) => void;
  readonly __wbg_utxocandidate_free: (a: number, b: number) => void;
  readonly jsparams_prefix: (a: number) => [number, number];
  readonly my_ids: (a: number, b: number) => [number, number, number];
  readonly network_params: (a: number, b: number) => [number, number, number];
  readonly __wbg_set_utxocandidate_spk_hex: (a: number, b: number, c: number) => void;
  readonly __wbg_get_utxocandidate_spk_hex: (a: number) => [number, number];
  readonly __wbg_get_utxocandidate_txid_hex: (a: number) => [number, number];
  readonly claim_tx: (a: number, b: number, c: number, d: number, e: number, f: bigint, g: number, h: number, i: bigint, j: number, k: number, l: number, m: number, n: bigint, o: number, p: number, q: bigint, r: number, s: number, t: number, u: number, v: bigint) => [number, number, number, number];
  readonly create_game_tx: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: bigint, l: bigint, m: number, n: number, o: bigint, p: number, q: number) => [number, number, number, number];
  readonly join_game_tx: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number, l: number, m: bigint, n: number, o: number) => [number, number, number, number];
  readonly transfer_tx: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number, l: bigint, m: number, n: number) => [number, number, number, number];
  readonly turn_tx: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number, l: number, m: number, n: number, o: number) => [number, number, number, number];
  readonly withdraw_tx: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: bigint) => [number, number, number, number];
  readonly sys_verify_integrity: (a: number, b: number) => void;
  readonly sys_write: (a: number, b: number, c: number) => void;
  readonly sys_cycle_count: () => bigint;
  readonly sys_input: (a: number) => number;
  readonly sys_log: (a: number, b: number) => void;
  readonly sys_rand: (a: number, b: number) => void;
  readonly syscall_2_nr: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => void;
  readonly sys_halt: (a: number, b: number) => void;
  readonly sys_pause: (a: number, b: number) => void;
  readonly sys_prove_keccak: (a: number, b: number) => void;
  readonly sys_verify_integrity2: (a: number, b: number) => void;
  readonly sys_read: (a: number, b: number, c: number) => number;
  readonly sys_panic: (a: number, b: number) => void;
  readonly sys_read_words: (a: number, b: number, c: number) => number;
  readonly __wbg_get_nodedescriptor_uid: (a: number) => [number, number];
  readonly __wbg_get_nodedescriptor_url: (a: number) => [number, number];
  readonly __wbg_nodedescriptor_free: (a: number, b: number) => void;
  readonly __wbg_set_nodedescriptor_uid: (a: number, b: number, c: number) => void;
  readonly __wbg_set_nodedescriptor_url: (a: number, b: number, c: number) => void;
  readonly __wbg_transaction_free: (a: number, b: number) => void;
  readonly transaction_addresses: (a: number, b: any) => [number, number, number];
  readonly transaction_constructor: (a: any) => [number, number, number];
  readonly transaction_deserializeFromJSON: (a: number, b: number) => [number, number, number];
  readonly transaction_deserializeFromObject: (a: any) => [number, number, number];
  readonly transaction_deserializeFromSafeJSON: (a: number, b: number) => [number, number, number];
  readonly transaction_finalize: (a: number) => [number, number, number];
  readonly transaction_gas: (a: number) => bigint;
  readonly transaction_get_inputs_as_js_array: (a: number) => any;
  readonly transaction_get_mass: (a: number) => bigint;
  readonly transaction_get_outputs_as_js_array: (a: number) => any;
  readonly transaction_get_payload_as_hex_string: (a: number) => [number, number];
  readonly transaction_get_subnetwork_id_as_hex: (a: number) => [number, number];
  readonly transaction_id: (a: number) => [number, number];
  readonly transaction_is_coinbase: (a: number) => number;
  readonly transaction_lockTime: (a: number) => bigint;
  readonly transaction_populateGenesisCovenants: (a: number, b: any) => [number, number];
  readonly transaction_serializeToJSON: (a: number) => [number, number, number, number];
  readonly transaction_serializeToObject: (a: number) => [number, number, number];
  readonly transaction_serializeToSafeJSON: (a: number) => [number, number, number, number];
  readonly transaction_set_gas: (a: number, b: bigint) => void;
  readonly transaction_set_inputs_from_js_array: (a: number, b: any) => void;
  readonly transaction_set_lockTime: (a: number, b: bigint) => void;
  readonly transaction_set_mass: (a: number, b: bigint) => void;
  readonly transaction_set_outputs_from_js_array: (a: number, b: any) => void;
  readonly transaction_set_payload_from_js_value: (a: number, b: any) => void;
  readonly transaction_set_subnetwork_id_from_js_value: (a: number, b: any) => void;
  readonly transaction_set_version: (a: number, b: number) => void;
  readonly transaction_version: (a: number) => number;
  readonly transaction_set_storage_mass: (a: number, b: bigint) => void;
  readonly transaction_get_storage_mass: (a: number) => bigint;
  readonly __wbg_get_utxoentry_address: (a: number) => number;
  readonly __wbg_get_utxoentry_amount: (a: number) => bigint;
  readonly __wbg_get_utxoentry_blockDaaScore: (a: number) => bigint;
  readonly __wbg_get_utxoentry_covenantId: (a: number) => number;
  readonly __wbg_get_utxoentry_isCoinbase: (a: number) => number;
  readonly __wbg_get_utxoentry_outpoint: (a: number) => number;
  readonly __wbg_get_utxoentry_scriptPublicKey: (a: number) => number;
  readonly __wbg_set_utxoentry_address: (a: number, b: number) => void;
  readonly __wbg_set_utxoentry_amount: (a: number, b: bigint) => void;
  readonly __wbg_set_utxoentry_blockDaaScore: (a: number, b: bigint) => void;
  readonly __wbg_set_utxoentry_covenantId: (a: number, b: number) => void;
  readonly __wbg_set_utxoentry_isCoinbase: (a: number, b: number) => void;
  readonly __wbg_set_utxoentry_outpoint: (a: number, b: number) => void;
  readonly __wbg_set_utxoentry_scriptPublicKey: (a: number, b: number) => void;
  readonly __wbg_utxoentries_free: (a: number, b: number) => void;
  readonly __wbg_utxoentry_free: (a: number, b: number) => void;
  readonly __wbg_utxoentryreference_free: (a: number, b: number) => void;
  readonly utxoentries_amount: (a: number) => bigint;
  readonly utxoentries_get_items_as_js_array: (a: number) => any;
  readonly utxoentries_js_ctor: (a: any) => [number, number, number];
  readonly utxoentries_set_items_from_js_array: (a: number, b: any) => void;
  readonly utxoentries_sort: (a: number) => void;
  readonly utxoentry_toString: (a: number) => [number, number, number];
  readonly utxoentryreference_address: (a: number) => number;
  readonly utxoentryreference_amount: (a: number) => bigint;
  readonly utxoentryreference_blockDaaScore: (a: number) => bigint;
  readonly utxoentryreference_entry: (a: number) => number;
  readonly utxoentryreference_isCoinbase: (a: number) => number;
  readonly utxoentryreference_outpoint: (a: number) => number;
  readonly utxoentryreference_scriptPublicKey: (a: number) => number;
  readonly utxoentryreference_toString: (a: number) => [number, number, number];
  readonly __wbg_transactionoutpoint_free: (a: number, b: number) => void;
  readonly __wbg_transactioninput_free: (a: number, b: number) => void;
  readonly transactioninput_constructor: (a: any) => [number, number, number];
  readonly transactioninput_get_compute_budget: (a: number) => number;
  readonly transactioninput_get_previous_outpoint: (a: number) => number;
  readonly transactioninput_get_sequence: (a: number) => bigint;
  readonly transactioninput_get_sig_op_count: (a: number) => number;
  readonly transactioninput_get_signature_script_as_hex: (a: number) => [number, number];
  readonly transactioninput_get_utxo: (a: number) => number;
  readonly transactioninput_set_compute_budget: (a: number, b: number) => void;
  readonly transactioninput_set_previous_outpoint: (a: number, b: any) => [number, number];
  readonly transactioninput_set_sequence: (a: number, b: bigint) => void;
  readonly transactioninput_set_sig_op_count: (a: number, b: number) => void;
  readonly transactioninput_set_signature_script_from_js_value: (a: number, b: any) => [number, number];
  readonly __wbg_covenantbinding_free: (a: number, b: number) => void;
  readonly __wbg_genesiscovenantgroup_free: (a: number, b: number) => void;
  readonly __wbg_transactionoutput_free: (a: number, b: number) => void;
  readonly covenantbinding_authorizingInput: (a: number) => number;
  readonly covenantbinding_covenantId: (a: number) => number;
  readonly covenantbinding_new: (a: number, b: number) => number;
  readonly covenantbinding_set_authorizingInput: (a: number, b: number) => void;
  readonly covenantbinding_set_covenantId: (a: number, b: number) => void;
  readonly covenantbinding_toJSON: (a: number) => [number, number, number];
  readonly genesiscovenantgroup_authorizingInput: (a: number) => number;
  readonly genesiscovenantgroup_ctor: (a: number, b: any) => [number, number, number];
  readonly genesiscovenantgroup_outputs: (a: number) => any;
  readonly genesiscovenantgroup_set_authorizingInput: (a: number, b: number) => void;
  readonly genesiscovenantgroup_set_outputs: (a: number, b: any) => [number, number];
  readonly genesiscovenantgroup_toJSON: (a: number) => [number, number, number];
  readonly genesiscovenantgroup_toString: (a: number) => [number, number, number];
  readonly transactionoutput_covenant: (a: number) => number;
  readonly transactionoutput_ctor: (a: bigint, b: number, c: number) => number;
  readonly transactionoutput_scriptPublicKey: (a: number) => number;
  readonly transactionoutput_set_covenant: (a: number, b: number) => void;
  readonly transactionoutput_set_scriptPublicKey: (a: number, b: number) => void;
  readonly transactionoutput_set_value: (a: number, b: bigint) => void;
  readonly transactionoutput_value: (a: number) => bigint;
  readonly sys_sha_buffer: (a: number, b: number, c: number, d: number) => void;
  readonly sys_sha_compress: (a: number, b: number, c: number, d: number) => void;
  readonly sys_alloc_aligned: (a: number, b: number) => number;
  readonly sys_alloc_words: (a: number) => number;
  readonly sys_argc: () => number;
  readonly sys_argv: (a: number, b: number, c: number) => number;
  readonly sys_bigint: (a: number, b: number, c: number, d: number, e: number) => void;
  readonly sys_bigint2_1: (a: number, b: number) => void;
  readonly sys_bigint2_2: (a: number, b: number, c: number) => void;
  readonly sys_bigint2_3: (a: number, b: number, c: number, d: number) => void;
  readonly sys_bigint2_4: (a: number, b: number, c: number, d: number, e: number) => void;
  readonly sys_bigint2_5: (a: number, b: number, c: number, d: number, e: number, f: number) => void;
  readonly sys_bigint2_6: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => void;
  readonly sys_exit: (a: number) => void;
  readonly sys_fork: () => number;
  readonly sys_getenv: (a: number, b: number, c: number, d: number) => number;
  readonly sys_keccak: (a: number, b: number) => number;
  readonly sys_pipe: (a: number) => number;
  readonly sys_poseidon2: (a: number, b: number, c: number, d: number) => void;
  readonly syscall_0: (a: number, b: number, c: number, d: number) => void;
  readonly syscall_0_nr: (a: number, b: number, c: number, d: number, e: number) => void;
  readonly syscall_1: (a: number, b: number, c: number, d: number, e: number) => void;
  readonly syscall_1_nr: (a: number, b: number, c: number, d: number, e: number, f: number) => void;
  readonly syscall_2: (a: number, b: number, c: number, d: number, e: number, f: number) => void;
  readonly syscall_3: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => void;
  readonly syscall_3_nr: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number) => void;
  readonly syscall_4: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number) => void;
  readonly syscall_4_nr: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number) => void;
  readonly syscall_5: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number) => void;
  readonly syscall_5_nr: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number) => void;
  readonly __wbg_get_networkid_suffix: (a: number) => number;
  readonly __wbg_get_networkid_type: (a: number) => number;
  readonly __wbg_networkid_free: (a: number, b: number) => void;
  readonly __wbg_set_networkid_suffix: (a: number, b: number) => void;
  readonly __wbg_set_networkid_type: (a: number, b: number) => void;
  readonly networkid_addressPrefix: (a: number) => [number, number];
  readonly networkid_ctor: (a: any) => [number, number, number];
  readonly networkid_id: (a: number) => [number, number];
  readonly networkid_toString: (a: number) => [number, number];
  readonly __wbg_get_scriptpublickey_version: (a: number) => number;
  readonly __wbg_get_transactionutxoentry_amount: (a: number) => bigint;
  readonly __wbg_get_transactionutxoentry_blockDaaScore: (a: number) => bigint;
  readonly __wbg_get_transactionutxoentry_covenantId: (a: number) => number;
  readonly __wbg_get_transactionutxoentry_isCoinbase: (a: number) => number;
  readonly __wbg_get_transactionutxoentry_scriptPublicKey: (a: number) => number;
  readonly __wbg_scriptpublickey_free: (a: number, b: number) => void;
  readonly __wbg_set_scriptpublickey_version: (a: number, b: number) => void;
  readonly __wbg_set_transactionutxoentry_amount: (a: number, b: bigint) => void;
  readonly __wbg_set_transactionutxoentry_blockDaaScore: (a: number, b: bigint) => void;
  readonly __wbg_set_transactionutxoentry_covenantId: (a: number, b: number) => void;
  readonly __wbg_set_transactionutxoentry_isCoinbase: (a: number, b: number) => void;
  readonly __wbg_set_transactionutxoentry_scriptPublicKey: (a: number, b: number) => void;
  readonly __wbg_transactionutxoentry_free: (a: number, b: number) => void;
  readonly scriptpublickey_constructor: (a: number, b: any) => [number, number, number];
  readonly scriptpublickey_script_as_hex: (a: number) => [number, number];
  readonly __wbg_sighashtype_free: (a: number, b: number) => void;
  readonly rustsecp256k1_v0_10_0_default_error_callback_fn: (a: number, b: number) => void;
  readonly rustsecp256k1_v0_10_0_default_illegal_callback_fn: (a: number, b: number) => void;
  readonly rustsecp256k1_v0_10_0_context_destroy: (a: number) => void;
  readonly rustsecp256k1_v0_10_0_context_create: (a: number) => number;
  readonly __wbg_hash_free: (a: number, b: number) => void;
  readonly hash_constructor: (a: number, b: number) => number;
  readonly hash_toString: (a: number) => [number, number];
  readonly address_constructor: (a: number, b: number) => number;
  readonly address_payload: (a: number) => [number, number];
  readonly address_prefix: (a: number) => [number, number];
  readonly address_set_setPrefix: (a: number, b: number, c: number) => void;
  readonly address_toString: (a: number) => [number, number];
  readonly address_validate: (a: number, b: number) => number;
  readonly address_version: (a: number) => [number, number];
  readonly __wbg_address_free: (a: number, b: number) => void;
  readonly defer: () => any;
  readonly initWASM32Bindings: (a: any) => [number, number];
  readonly initBrowserPanicHook: () => void;
  readonly initConsolePanicHook: () => void;
  readonly presentPanicHookLogs: () => void;
  readonly __wbg_abortable_free: (a: number, b: number) => void;
  readonly __wbg_aborted_free: (a: number, b: number) => void;
  readonly abortable_abort: (a: number) => void;
  readonly abortable_check: (a: number) => [number, number];
  readonly abortable_isAborted: (a: number) => number;
  readonly abortable_new: () => number;
  readonly abortable_reset: (a: number) => void;
  readonly setLogLevel: (a: any) => void;
  readonly __wbindgen_exn_store: (a: number) => void;
  readonly __externref_table_alloc: () => number;
  readonly __wbindgen_export_2: WebAssembly.Table;
  readonly __wbindgen_free: (a: number, b: number, c: number) => void;
  readonly __wbindgen_malloc: (a: number, b: number) => number;
  readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
  readonly __externref_table_dealloc: (a: number) => void;
  readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;
/**
* Instantiates the given `module`, which can either be bytes or
* a precompiled `WebAssembly.Module`.
*
* @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
*
* @returns {InitOutput}
*/
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
* If `module_or_path` is {RequestInfo} or {URL}, makes a request and
* for everything else, calls `WebAssembly.instantiate` directly.
*
* @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
*
* @returns {Promise<InitOutput>}
*/
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
