require 'utilities/hash'

class Sendgrid::UpdateEmailJob < SendgridJob
  def perform(old_email, new_email)
    old_contact_details = Rails.cache.fetch("sendgrid_update_email_job_contact_details_#{old_email}", expires_in: 30.days) do
      result = client.get_contacts_by_emails(emails: [old_email])
      contact_name = Utilities::Hash.dig_or_raise(result.data, 'result', old_email, 'contact', 'first_name')
      contact_id = Utilities::Hash.dig_or_raise(result.data, 'result', old_email, 'contact', 'id')
      contact_lists_ids = Utilities::Hash.dig_or_raise(result.data, 'result', old_email, 'contact', 'list_ids')
      {
        contact_name: contact_name,
        contact_id: contact_id,
        contact_lists_ids: contact_lists_ids
      }
    end

    result = client.delete_contacts(ids: [old_contact_details[:contact_id]])
    raise StandardError, result.errors if result.failure?

    contact = {
      email: new_email,
      first_name: old_contact_details[:contact_name]
    }.compact

    result = client.add_or_update_contacts(
      list_ids: old_contact_details[:contact_lists_ids],
      contacts: [contact]
    )
    raise StandardError, result.errors if result.failure?

    Rails.cache.delete("sendgrid_update_email_job_contact_details_#{old_email}")
  end
end
