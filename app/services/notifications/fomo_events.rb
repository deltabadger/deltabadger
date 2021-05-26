require 'fomo'

module Notifications
  class FomoEvents
    AUTHORIZATION_TOKEN = ENV.fetch('FOMO_AUTH_TOKEN')
    INVESTOR_TEMPLATE_ID = 149849
    HODLER_TEMPLATE_ID = 149922

    def initialize
      @client = Fomo.new(AUTHORIZATION_TOKEN)
    end

    def plan_bought(first_name:, country:, plan_name:)
      event = FomoEvent.new
      event.event_type_id = plan_name == 'hodler' ? HODLER_TEMPLATE_ID : INVESTOR_TEMPLATE_ID
      event.first_name = first_name
      event.ip_address = country
      #event.country = country

      @client.create_event(event)
    end
  end
end
