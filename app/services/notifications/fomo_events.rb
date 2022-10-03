require 'fomo'

module Notifications
  class FomoEvents
    AUTHORIZATION_TOKEN = ENV.fetch('FOMO_AUTH_TOKEN')
    INVESTOR_TEMPLATE_ID = 149_849
    HODLER_TEMPLATE_ID = 149_922
    LEGENDARY_BADGER_TEMPLATE_ID = 171_637

    def initialize
      @client = Fomo.new(AUTHORIZATION_TOKEN)
    end

    def plan_bought(first_name:, country: nil, ip_address: nil, plan_name:)
      event = FomoEvent.new
      event.event_type_id = if plan_name == 'hodler'
        HODLER_TEMPLATE_ID
      elsif plan_name == 'investor'
        INVESTOR_TEMPLATE_ID
      else
        LEGENDARY_BADGER_TEMPLATE_ID
      end
      event.first_name = first_name
      event.ip_address = ip_address unless ip_address.nil?
      event.country = country unless country.nil?

      @client.create_event(event)
    end
  end
end
