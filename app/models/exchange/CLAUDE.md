## DRY_RUN Mode

- Set `DRY_RUN=true` in `.env` for development
- When enabled, exchanges return mocked data instead of real API calls
- Production always forces `DRY_RUN=false`
- Test always forces `DRY_RUN=true`
- Implemented in `dryable.rb`
