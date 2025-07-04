class Upgrade::InstructionsController < ApplicationController
  before_action :authenticate_user!

  def show
    # FIXME: we need this sleep to workaround the situation when a rendering the upgrade_instructions modal
    # is called from another modal (e.g. the new barbell bot creation for a free user with other bots created).
    # The problem is that turbo has not yet cleaned up the modal object and it tries to render this modal
    # into the same modal partial, and crashes.
    # Seems the issue is actually related to the modal--base#animateOutCloseAndCleanUp action, which is triggered
    # but not awaited to finish before rendering the modal.
    # The FIX must address both upgrade_instructions_path and new_bots_dca_dual_assets_pick_first_buyable_asset_path.
    sleep 0.25
  end
end
