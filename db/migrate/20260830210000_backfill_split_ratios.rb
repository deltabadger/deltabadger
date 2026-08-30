# Splits already on record were stored before the ratio and the corporate-action marker existed,
# and a sync can never revisit them: it resumes near its watermark, and even a full re-fetch is
# discarded by the duplicate guard on `tx_id`. So they are rewritten in place.
#
# Both counts are recoverable from what was kept: `raw_data['qty']` is the leg the merge happened
# to lead with, and `base_amount` is the net delta, so whichever leg it was, the other side is
# that leg plus or minus the delta.
class BackfillSplitRatios < ActiveRecord::Migration[8.1]
  def up
    AccountTransaction.where(entry_type: :adjustment).find_each do |at|
      raw = at.raw_data
      next unless raw.is_a?(Hash) && Exchanges::Alpaca::SPLIT_TYPES.include?(raw['activity_type'])
      next if raw['corporate_action'] == 'split' # already rewritten — never append a ratio twice

      ratio = ratio_for(at, raw)
      at.update_columns(
        raw_data: raw.merge({ 'corporate_action' => 'split', 'split_ratio' => ratio }.compact),
        description: [at.description.presence, ratio].compact.join(' ').presence
      )
      AccountTransactionSync.announce_split(user: at.user, exchange: at.exchange,
                                            symbol: at.base_currency, at: at.transacted_at, ratio: ratio)
    end
  end

  def down
    # Nothing to restore: this only adds what the rows should always have carried.
  end

  private

  def ratio_for(account_transaction, raw)
    leg = raw['qty'].to_d
    net = account_transaction.base_amount.to_d
    old_count, new_count = leg.negative? ? [-leg, -leg + net] : [leg - net, leg]
    Exchanges::Alpaca.split_ratio_label(old_count, new_count)
  end
end
