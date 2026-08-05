# Instance-wide configuration (market data provider, API keys shared by every user,
# email settings) must only be writable by the admin. Controllers that expose such a
# write include this and list the actions.
module AdminOnly
  extend ActiveSupport::Concern

  private

  def require_admin!
    head(:forbidden) unless current_user&.admin?
  end
end
