class BackfillMultiAssetConditionDefaults < ActiveRecord::Migration[8.1]
  # The trading-condition concerns added to Bots::DcaMultiAsset each write defaults in
  # after_initialize. On a row saved before they existed, that dirties `settings` the moment the row
  # is loaded, and Bot::Accountable raises on the next save unless set_missed_quote_amount was
  # called first (accountable.rb:82). Background paths that save without that call would start
  # failing on rows that were fine yesterday. Writing the defaults once here means a loaded row is
  # already clean.
  #
  # update_columns, NOT save: set_missed_quote_amount recomputes and persists the carry, and a
  # backfill of inert defaults must never move a bot's financial state. This writes exactly the
  # settings hash the model just initialized and nothing else — no validations, no callbacks.
  #
  # Referencing the model is safe: Bots::DcaMultiAsset is the type that survives this merge.
  def up
    Bots::DcaMultiAsset.find_each do |bot|
      next if bot.settings_was == bot.settings

      bot.update_columns(settings: bot.settings)
    end
  end

  def down
    # Defaults are inert values; leaving them costs nothing and removing them would re-open the trap.
  end
end
