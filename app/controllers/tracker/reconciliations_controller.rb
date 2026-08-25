# Settling a finding by writing the entry that is missing — with the user's explicit consent.
#
# `new` states the whole of what would be written: which coin, how much, dated when, and what it
# would be recorded as costing. `create` writes exactly that and nothing else. There is no path that
# writes without passing through the dialog, and no default that writes on its own: an opening
# balance changes a tax position, so it is the user's assertion, never ours.
class Tracker::ReconciliationsController < ApplicationController
  before_action :authenticate_user!
  before_action :load_proposal

  # The dialog states money, so it needs the user's own denominator — the tracker page sets this on
  # its own request and this one is its own.
  before_action { @denomination = current_user.denomination }

  def new
    render :new, layout: false
  end

  def create
    # `cost` is what the user answered, and `estimated` says whether they took our price or their
    # own — the tax report has to be able to tell those apart.
    # Typed in the currency they were shown; stored in the one everything behind the page uses.
    cost = @denomination.to_usd(params[:cost].presence)
    Tracker::Reconciliation.accept!(current_user, @symbol,
                                    arrival: params[:arrival] == 'earned' ? :earned : :bought,
                                    cost: cost, estimated: params[:source] == 'market')
    # The ledger is a reading of the transactions, and one just changed.
    Tracker::LedgerJob.perform_later(current_user.id)
    # Never "done" when it is not: this entry closes what it closes, and where the arithmetic leaves
    # something over, the dialog already said so and the message says it again.
    flash.now[:notice] = if @proposal.residual
                           t('tracker.findings.recorded_partly', symbol: @symbol,
                                                                 quantity: helpers.tracker_amount(@proposal.residual.abs))
                         else
                           t('tracker.findings.recorded', symbol: @symbol)
                         end
    render turbo_stream: [turbo_stream_prepend_flash, turbo_stream_redirect(tracker_path)]
  end

  private

  def load_proposal
    @symbol = params[:symbol].to_s
    @proposal = Tracker::Reconciliation.propose(current_user, @symbol)
    return if @proposal

    # Nothing to settle — the finding cleared while the dialog was open, or the symbol was invented.
    redirect_to tracker_path
  end
end
