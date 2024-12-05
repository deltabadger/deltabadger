class SendgridJob < ApplicationJob
  queue_as :sendgrid

  private

  def client
    @client ||= SendgridClient.new
  end

  def get_list_id(list_name)
    result = client.get_all_lists
    result.data.fetch('result').select { |list| list['name'] == list_name }.pluck('id').first
  end
end
