# Refuse anything that would connect a key to a venue that no longer exists (see
# Exchange::RETIRED_TYPES). ApiKey's own validation is the hard stop, but it cannot produce a useful
# message here: every API-key controller replaces model errors with generic credential copy
# ("we couldn't verify your key"), which would be misleading for a venue that shut down. So the
# guard runs before the credential flow does.
module RetiredExchangeGuard
  extend ActiveSupport::Concern

  private

  # Returns true when the request has been answered, so callers can `return if ...`.
  #
  # redirect_to, NOT redirect_back: a credential form opened before the venue was retired POSTs with
  # the now-rejected `new` page as its referrer, so redirect_back would bounce the user straight
  # back into a page that rejects them again.
  def reject_retired_exchange(exchange, fallback: root_path)
    return false unless exchange&.retired?

    flash[:alert] = I18n.t('errors.exchange_retired')
    redirect_to fallback
    true
  end
end
