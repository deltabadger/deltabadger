# All Administrate controllers inherit from this `Admin::ApplicationController`,
# making it the ideal place to put authentication logic or other
# before_actions.
#
# If you want to add pagination or other controller-level concerns,
# you're free to overwrite the RESTful controller actions.
module Admin
  class ApplicationController < Administrate::ApplicationController
    before_action :authenticate_user!
    before_action :authenticate_admin
    before_action :redirect_to_syncing_if_needed

    def authenticate_admin
      redirect_to root_path if !current_user.admin?
    end

    def redirect_to_syncing_if_needed
      return unless AppConfig.setup_sync_needed?

      redirect_to setup_syncing_path
    end

    def model_name
      raise NotImplementedError
    end

    def default_sorting_attribute
      get_sorting_attribute(model_name)
    end

    def default_sorting_direction
      :desc
    end

    private

    def get_sorting_attribute(model_name)
      %i[user].include?(model_name) ? :created_at : :id
    end

    # Override this value to specify the number of elements to display at a time
    # on index pages. Defaults to 20.
    # def records_per_page
    #   params[:per_page] || 20
    # end
  end
end
