# frozen_string_literal: true

module BotApi
  module Tracker
    # Linking a transfer changes whether the coins ever left the portfolio, and the snapshots that
    # said they did are now wrong. Coverage cannot notice — the dates did not move — so the one
    # action that edits history asks for the rebuild itself.
    class SetTransferLink
      def self.call(user:, transaction_id:, linked:)
        new(user: user, transaction_id: transaction_id, linked: linked).call
      end

      def initialize(user:, transaction_id:, linked:)
        @user = user
        @transaction_id = transaction_id
        # Strict: a cast that turned nil into false would make "linked omitted" an unlink that
        # rewrites history.
        @linked = Boolean.parse(linked)
      end

      def call
        return Result.failure(:validation_failed, 'linked_required', 'linked must be true or false.') if @linked.nil?

        transaction = AccountTransaction.for_user(@user).find_by(id: @transaction_id.to_i)
        return Result.failure(:not_found, 'transaction_not_found', 'Transaction not found.') unless transaction

        result = @linked ? link(transaction) : unlink(transaction)
        if result.success?
          # History and the current reading both changed; backfill has early returns, so the
          # ledger is asked for directly rather than trusted to follow.
          PortfolioSnapshot::BackfillJob.perform_later(@user.id)
          ::Tracker::LedgerJob.perform_later(@user.id)
        end
        result
      end

      private

      def link(transaction)
        return Result.failure(:conflict, 'already_linked', 'This transaction is already linked to a transfer.') if transaction.linked?

        candidates = transaction.transfer_candidates(@user)
        case candidates.size
        when 0
          Result.failure(:not_found, 'no_transfer_candidate', 'No matching transfer within 14 days.')
        when 1
          withdrawal, deposit = transaction.withdrawal? ? [transaction, candidates.first] : [candidates.first, transaction]
          withdrawal.update!(linked_transaction_id: deposit.id, transfer_link_rejected: false)
          Result.success({ withdrawal_id: withdrawal.id, deposit_id: deposit.id, linked: true })
        else
          Result.failure(:conflict, 'ambiguous_transfer_candidate',
                         'More than one transfer matches; link it from the tracker.')
        end
      rescue ActiveRecord::RecordNotUnique
        # Two requests raced for the same deposit; the other one won. The unique index on
        # linked_transaction_id is the lock, and this is its message rather than a 500.
        Result.failure(:conflict, 'already_linked', 'That transfer was linked by a concurrent request.')
      end

      def unlink(transaction)
        return Result.failure(:conflict, 'not_linked', 'This transaction is not linked to a transfer.') unless transaction.linked?

        withdrawal = transaction.withdrawal? ? transaction : transaction.inverse_link
        deposit_id = withdrawal.linked_transaction_id
        # Sticky, or TransferMatcher re-links the pair on the next sync and the undo silently
        # undoes itself.
        withdrawal.update!(linked_transaction_id: nil, transfer_link_rejected: true)
        Result.success({ withdrawal_id: withdrawal.id, deposit_id: deposit_id, linked: false })
      end
    end
  end
end
