## Order Flow

1. Bot scheduled via `Bot::ActionJob`
2. Job calls `bot.execute_action`
3. Concern decorators modify behavior (limits, smart intervals, etc.)
4. `OrderCreator#set_order` creates Transaction
5. Exchange API called to submit order
6. `Bot::FetchAndUpdateOrderJob` polls for completion
7. Turbo Streams broadcast updates to UI

## Real-Time Updates

- Turbo Streams broadcast to user-specific channels: `["user_#{user_id}", :bot_updates]`
- Broadcast methods on Bot: `broadcast_status_bar_update`, `broadcast_new_order`
