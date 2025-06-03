# SOL Flashloan bot

A Solana flash loan bot is an automated trading tool that leverages flash loans—uncollateralized, instant loans that must be borrowed and repaid within a single transaction—to exploit arbitrage opportunities, liquidate undercollateralized positions, or perform complex DeFi strategies on Solana. Unlike Ethereum, Solana’s high-speed, low-fee environment enables flash loans to execute in milliseconds, making them ideal for high-frequency trading (HFT) and MEV (Maximal Extractable Value) strategies.

## Configuration

Copy the `.env.example` file to `.env` and update the following key settings:

```
# Core settings
PRIVATE_KEY=your_solana_private_key
DEFAULT_RPC=your_rpc_endpoint

# Trading parameters
TRADE_SIZE_SOL=0.1                # Size of each trade in SOL
MIN_PROFIT_THRESHOLD=0.5          # Minimum profit percentage to execute a trade
MAX_SLIPPAGE_PERCENT=0.1          # Maximum allowed slippage in percent

# Thread and rotation settings
THREAD_AMOUNT=2                   # Number of worker threads
IS_USE_TOKEN_ROTATION=true        # Enable token rotation
TOKEN_ROTATION_CHECK_FREQUENCY=50 # How often to check for rotation (in number of scans)
TOKEN_ROTATION_INTERVAL_MINUTES=5 # How often to rotate tokens (time-based)
```

## How Solana Flash Loans Work

- Atomic Execution: Flash loans on Solana are executed atomically within a single transaction, meaning if repayment fails, the entire transaction reverts.

- No Collateral Required: Unlike traditional loans, flash loans do not require upfront collateral, as they are borrowed and repaid in the same block.

- Smart Contract Integration: Flash loans rely on Solana programs (smart contracts) like Raydium, Solend, or Flash Loan Pools to facilitate borrowing and repayment.

- High-Speed Processing: Solana’s 400ms block time allows flash loan bots to execute multiple trades before Ethereum even processes a single block.

- The bot supports two token rotation modes that you can configure with the `IS_USE_TOKEN_ROTATION` option:

## Key Strategies for Solana Flash Loan Bots

- DEX Arbitrage: Exploiting price differences between Serum, Orca, and Raydium.

- Liquidation Hunting: Repaying undercollateralized loans in lending protocols (e.g., Solend, Port Finance) for rewards.

- Token Swaps with Leverage: Using flash loans to amplify trading positions without capital.

- NFT Flipping: Flash-borrowing SOL to snipe undervalued NFTs and resell them instantly.

## Technical Stack for Building a Solana Flash Loan Bot

- Programming Languages: Rust (for Solana programs), Python/TypeScript (for bot logic).

- Solana SDKs: Anchor Framework, Solana Web3.js, Solana-py.

- APIs & Tools: Jupiter Aggregator API, Pyth Network for pricing, Jito for MEV optimization.

- Wallets: Phantom (for testing), with programmatic key management.

## Running the Bot

```bash
cargo run --release
```

## Command Line Options

The bot supports several command line options:

```bash
cargo run --release -- --size 0.1 --profit 0.5 --interval 5 --threads 2
```

- `--size`: Trade size in SOL
- `--profit`: Minimum profit threshold percentage
- `--interval`: Token rotation interval in minutes
- `--threads`: Number of worker threads

## Trading Strategies

The bot supports two trading size strategies:

### Fixed Trade Size

When `USE_DYNAMIC_TRADE_SIZE=false` (default), the bot uses a fixed trade size specified by `TRADE_SIZE_SOL`.

### Dynamic Trade Size

When `USE_DYNAMIC_TRADE_SIZE=true`, the bot calculates an optimal trade size based on the profit percentage, adapting to market conditions.

## Performance Optimizations

The bot includes several performance optimizations for efficient market scanning:

1. **Early Exit for Unprofitable Tokens**: The bot quickly abandons tokens with negative profit percentage, allowing it to rapidly scan through more tokens.

2. **Sequential Token Rotation** (when `IS_USE_TOKEN_ROTATION=false`): The bot cycles through tokens every 5 scans, providing comprehensive market coverage.

3. **Quote Manufacturing**: Reduces API calls by mathematically modeling price impact for larger trades based on small trade quotes.
