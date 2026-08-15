# Auto-links withdrawal->deposit pairs that look like the user's own transfers so tax
# engines can skip them. Conservative: same asset, deposit within 72h after the
# withdrawal, amount shrinkage 0..2% (network fee). Never overwrites manual links.
class TransferMatcher
  WINDOW = 72.hours
  TOLERANCE = '0.02'.to_d

  def self.run!(user)
    linked_deposit_ids = AccountTransaction.for_user(user).where.not(linked_transaction_id: nil)
                                           .pluck(:linked_transaction_id)
    AccountTransaction.for_user(user).withdrawal.where(linked_transaction_id: nil).find_each do |withdrawal|
      withdrawal_amount = withdrawal.base_amount.to_d
      minimum_amount = withdrawal_amount * ('1'.to_d - TOLERANCE)
      candidate = AccountTransaction.for_user(user).deposit
                                    .where(base_currency: withdrawal.base_currency)
                                    .where(transacted_at: withdrawal.transacted_at..(withdrawal.transacted_at + WINDOW))
                                    .where.not(id: linked_deposit_ids)
                                    .where(base_amount: minimum_amount..withdrawal_amount)
                                    .order(:transacted_at).first
      next unless candidate

      withdrawal.update!(linked_transaction_id: candidate.id)
      linked_deposit_ids << candidate.id
    end
  end
end
