# SOL Dynamic Arbitrage Bot

This bot scans for arbitrage opportunities on Solana using Jupiter aggregator, supporting token rotation to scan through multiple pairs.

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

## Token Rotation

The bot supports two token rotation modes that you can configure with the `IS_USE_TOKEN_ROTATION` option:

### Mode 1: Managed Rotation (IS_USE_TOKEN_ROTATION=true)

When enabled, the bot uses a sophisticated token rotation system:

1. The bot fetches trending tokens from Jupiter API with high volume.
2. Each thread starts with a different token and cycles through all available tokens.
3. Rotation happens:
   - Every X scans (configured by the frequency in main.rs)
   - At least every Y minutes (configured by TOKEN_ROTATION_INTERVAL_MINUTES)
4. The system tracks which tokens have been used by each thread to prevent repeating the same tokens too frequently.

### Mode 2: Sequential Scan (IS_USE_TOKEN_ROTATION=false)

When disabled, the bot uses a simple sequential rotation:

1. The bot still fetches trending tokens from Jupiter API, but rotates through them more frequently.
2. Each thread cycles to a new token every 5 scans, going through the entire token list sequentially.
3. This provides rapid coverage of all tokens.

### Tuning Token Rotation

- For Mode 1 (managed): Adjust `TOKEN_ROTATION_INTERVAL_MINUTES` and token_rotation_check_frequency in main.rs
- For Mode 2 (sequential): The rotation frequency is fixed at every 5 scans
- For more parallel scanning: Increase `THREAD_AMOUNT` (each thread will handle different tokens)

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

You can enable/disable it using the USE_MANUFACTURED_QUOTES environment variable (default: true)
You can control the validation of large trades with VALIDATE_LARGE_QUOTES and VALIDATION_THRESHOLD_SOL
You can adjust the price impact model using PRICE_IMPACT_MODEL ("sqrt", "linear", "square", or "log")
You can validate the accuracy of quote manufacturing with --validate-quotes