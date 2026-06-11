# Liveness probe for the Docker HEALTHCHECK (Dockerfile) and any external monitor.
# Inherits ActionController::Base directly (NOT ApplicationController) so it runs none
# of the app's DB-touching before_actions (redirect_to_setup_if_needed, switch_locale).
# A liveness probe must never depend on the ActiveRecord connection pool — otherwise the
# in-Puma stock-sync job pinning the `primary` connection turns a healthy container into a
# 500 (see docs/superpowers/plans/2026-06-11-stock-sync-connection-pool-starvation.md).
class HealthCheckController < ActionController::Base
  def index
    render json: { health: 'check' }
  end
end
